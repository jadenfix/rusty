#!/usr/bin/env python3
"""Replay real PTY checkpoints: settings and cancellation must return a clean editor.
Captures use fixed ASCII inputs. Emoji terminal/font width is not qualified here.
"""
import json
from pathlib import Path
import re
import unicodedata

CSI=re.compile(r'\x1b\[([0-9;?]*)([ -/]*)([@-~])')

class Screen:
    def __init__(self,cols,rows):
        self.cols=cols;self.rows=rows;self.r=self.c=0
        self.grid=[[' ']*cols for _ in range(rows)]
        self.cursor=True;self.paste=False;self.pending=""
    def down(self):
        if self.r==self.rows-1:self.grid.pop(0);self.grid.append([' ']*self.cols)
        else:self.r+=1
    def feed(self,text):
        text=self.pending+text;self.pending=""
        i=0
        while i<len(text):
            ch=text[i]
            if ch=='\x1b':
                m=CSI.match(text,i)
                if not m:
                    if re.fullmatch(r'\x1b(?:\[[0-9;?]*[ -/]*)?',text[i:]):
                        self.pending=text[i:];return
                    raise AssertionError(('unsupported escape in capture',repr(text[i:i+30])))
                vals=m[1].lstrip('?').split(';');n=int(vals[0] or '0');n1=max(1,n);code=m[3]
                if code=='A':self.r=max(0,self.r-n1)
                elif code=='B':self.r=min(self.rows-1,self.r+n1)
                elif code=='C':self.c=min(self.cols-1,self.c+n1)
                elif code=='D':self.c=max(0,self.c-n1)
                elif code=='G':self.c=min(self.cols-1,n1-1)
                elif code in ['H','f']:
                    self.r=min(self.rows-1,n1-1);self.c=min(self.cols-1,max(1,int(vals[1] or '1'))-1 if len(vals)>1 else 0)
                elif code=='K':
                    if n==2:self.grid[self.r]=[' ']*self.cols
                    elif n==0:self.grid[self.r][min(self.c,self.cols):]=[' ']*max(0,self.cols-self.c)
                    elif n==1:self.grid[self.r][:self.c+1]=[' ']*(self.c+1)
                elif code=='J':
                    if n==2:self.grid=[[' ']*self.cols for _ in range(self.rows)]
                    elif n==0:
                        self.grid[self.r][min(self.c,self.cols):]=[' ']*max(0,self.cols-self.c)
                        for row in range(self.r+1,self.rows):self.grid[row]=[' ']*self.cols
                elif code in ['h','l']:
                    if m[1]=='?25':self.cursor=code=='h'
                    elif m[1]=='?2004':self.paste=code=='h'
                i=m.end();continue
            elif ch=='\r':self.c=0
            elif ch=='\n':self.down()
            elif ch=='\b':self.c=max(0,self.c-1)
            elif ord(ch)>=32 and ord(ch)!=127:
                if unicodedata.combining(ch):i+=1;continue
                if self.c>=self.cols:self.c=0;self.down()
                width=2 if unicodedata.east_asian_width(ch) in ['W','F'] else 1
                self.grid[self.r][self.c]=ch
                if width==2 and self.c+1<self.cols:self.grid[self.r][self.c+1]=''
                self.c+=width
            i+=1
    def lines(self):return [''.join(row) for row in self.grid]


def inspect(dest,cols,rows):
    chunks=[];offset=0
    for record in (dest/'events.tsv').read_bytes().splitlines(keepends=True):
        offset+=len(record);_,data=record.decode().strip().split('\t');chunks.append((offset,bytes.fromhex(data).decode('utf-8')))
    points=[line.split('\t') for line in (dest/'checkpoints.tsv').read_text().splitlines()]
    screen=Screen(cols,rows);index=0;checked=[]
    for end,label,_,_ in points:
        while index<len(chunks) and chunks[index][0]<=int(end):screen.feed(chunks[index][1]);index+=1
        if label=='cleared-draft':continue  # Input may not have been drained yet.
        lines=screen.lines()
        (dest/'last-checkpoint.txt').write_text('\n'.join(lines)+'\n')
        assert screen.cursor and screen.paste,(label,'editor cursor or bracketed paste not restored')
        assert lines[screen.r].rstrip()=='›' and screen.c==2,(label,'prompt/caret overlapped',screen.r,screen.c,lines)
        assert not any('rusty /' in line or any('\u2801'<=ch<='\u28ff' for ch in line) for line in lines),(label,'transient activity panel leaked into settings',lines)
        checked.append(label)
    for _,text in chunks[index:]:screen.feed(text)
    assert screen.cursor and not screen.paste,'terminal input mode still active after exit'
    assert not any('rusty /' in line for line in screen.lines()),'activity left at exit'
    assert b'--More--' not in (dest/'terminal.ansi').read_bytes(),'blocking completion pager returned'
    return {'width':cols,'height':rows,'checkpoints':len(checked),'passed':True}


if __name__=='__main__':
    import sys
    root=Path(sys.argv[1]);reports=[]
    manifest=json.loads((root/'checks.json').read_text())
    journeys=manifest['journeys']
    assert manifest['passed'] and len(journeys)==manifest['expected_journeys'],'incomplete navigation run'
    for case in journeys:
        dest=root/f"{case['case']}-{case['width']}x{case['height']}"
        result=inspect(dest,case['width'],case['height']);result['case']=case['case'];reports.append(result)
    (root/'screen-checks.json').write_text(json.dumps({'passed':True,'checks':reports},indent=2)+'\n')
    print(json.dumps({'passed':True,'journeys':len(reports),'checkpoints':sum(x['checkpoints'] for x in reports)}))
