//! A child process on a real pseudo-terminal, read without blocking.

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

pub struct Pty {
    master: File,
    child: Child,
    eof: bool,
}

/// What one read produced.
pub enum Chunk {
    Data(Vec<u8>),
    Timeout,
    Eof,
}

impl Pty {
    /// Starts `cmd` as the session leader of a new terminal `rows` by `cols`.
    pub fn spawn(mut cmd: Command, rows: u16, cols: u16) -> Result<Pty> {
        let (mut master, mut slave) = (-1, -1);
        // SAFETY: openpty writes two descriptors; the null name, termios and
        // winsize arguments are allowed.
        let rc = unsafe {
            libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
        };
        if rc != 0 {
            return Err(io::Error::last_os_error()).context("openpty");
        }
        // SAFETY: both descriptors were just opened and are owned here. Close
        // on exec, so only the copies on 0, 1 and 2 reach the child.
        let (master, slave) = unsafe {
            for fd in [master, slave] {
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
            (File::from_raw_fd(master), OwnedFd::from_raw_fd(slave))
        };
        resize(&master, rows, cols)?;
        cmd.stdin(Stdio::from(slave.try_clone()?)).stdout(Stdio::from(slave.try_clone()?)).stderr(Stdio::from(slave));
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let program = cmd.get_program().to_string_lossy().into_owned();
        let child = cmd.spawn().with_context(|| format!("starting {program}"))?;
        drop(cmd); // closes the parent's copies of the terminal side
                   // SAFETY: fcntl on a descriptor we own.
        unsafe {
            let fd = master.as_raw_fd();
            libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK);
        }
        Ok(Pty { master, child, eof: false })
    }

    /// Changes the terminal size; the kernel sends the child SIGWINCH.
    pub fn set_size(&self, rows: u16, cols: u16) -> Result<()> {
        resize(&self.master, rows, cols)
    }

    /// Waits up to `timeout` for output.
    pub fn read(&mut self, timeout: Duration) -> Result<Chunk> {
        if self.eof {
            return Ok(Chunk::Eof);
        }
        let mut fds = libc::pollfd { fd: self.master.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        // SAFETY: one valid pollfd.
        let n = unsafe { libc::poll(&mut fds, 1, ms) };
        if n < 0 {
            let e = io::Error::last_os_error();
            return if e.kind() == io::ErrorKind::Interrupted { Ok(Chunk::Timeout) } else { Err(e.into()) };
        }
        if n == 0 {
            return Ok(Chunk::Timeout);
        }
        let mut buf = vec![0; 65536];
        match self.master.read(&mut buf) {
            Ok(0) => {}
            Ok(n) => {
                buf.truncate(n);
                return Ok(Chunk::Data(buf));
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(Chunk::Timeout),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => return Ok(Chunk::Timeout),
            // Linux reports EIO once every terminal-side descriptor is closed.
            Err(_) => {}
        }
        self.eof = true;
        Ok(Chunk::Eof)
    }

    pub fn send(&mut self, bytes: &[u8]) -> Result<()> {
        let mut rest = bytes;
        while !rest.is_empty() {
            match self.master.write(rest) {
                Ok(n) => rest = &rest[n..],
                Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::Interrupted => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => return Err(e).context("writing to the terminal"),
            }
        }
        Ok(())
    }

    /// Waits for the child to exit, killing it after `timeout`.
    pub fn wait(&mut self, timeout: Duration) -> Result<ExitStatus> {
        let end = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= end {
                self.kill();
                bail!("the process did not exit within {timeout:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            self.kill();
        }
    }
}

/// Sets the terminal size the child sees.
pub fn resize(master: &File, rows: u16, cols: u16) -> Result<()> {
    let size = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: TIOCSWINSZ reads one winsize.
    if unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ as _, &size) } != 0 {
        return Err(io::Error::last_os_error()).context("TIOCSWINSZ");
    }
    Ok(())
}

/// Decodes UTF-8 that may be split across reads; bad bytes become U+FFFD.
#[derive(Default)]
pub struct Utf8 {
    pending: Vec<u8>,
}

impl Utf8 {
    pub fn decode(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut out = String::new();
        let mut rest = &self.pending[..];
        loop {
            match std::str::from_utf8(rest) {
                Ok(s) => {
                    out.push_str(s);
                    rest = &[];
                    break;
                }
                Err(e) => {
                    let (good, bad) = rest.split_at(e.valid_up_to());
                    out.push_str(std::str::from_utf8(good).expect("validated"));
                    match e.error_len() {
                        Some(n) => {
                            out.push('\u{fffd}');
                            rest = &bad[n..];
                        }
                        None => {
                            rest = bad; // an incomplete character; wait for the rest
                            break;
                        }
                    }
                }
            }
        }
        self.pending = rest.to_vec();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_split_across_reads() {
        let mut d = Utf8::default();
        let bytes = "a✓b".as_bytes();
        assert_eq!(d.decode(&bytes[..2]), "a");
        assert_eq!(d.decode(&bytes[2..]), "✓b");
        assert_eq!(d.decode(b"\xffc"), "\u{fffd}c");
    }

    #[test]
    fn runs_a_child_on_a_terminal() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "stty size; test -t 0 && echo tty"]);
        let mut pty = Pty::spawn(cmd, 12, 34).unwrap();
        let mut out = Vec::new();
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            match pty.read(Duration::from_millis(200)).unwrap() {
                Chunk::Data(d) => out.extend(d),
                Chunk::Timeout => {}
                Chunk::Eof => break,
            }
        }
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("12 34") && text.contains("tty"), "{text}");
        assert!(pty.wait(Duration::from_secs(5)).unwrap().success());
    }
}
