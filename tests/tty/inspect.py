#!/usr/bin/env python3
"""Inspect the actual expect captures on a small VT screen model.
Checks complete panel frames, scrollback, and cleanup; saves replay frames.
"""
import json
from pathlib import Path
import re
import sys
import unicodedata

CSI = re.compile(r'\x1b\[([0-9;?]*)([ -/]*)([@-~])')

def inspect(path, cols, require_scroll=True):
    text=path.read_bytes().decode('utf-8')
    grid=[[' ']*cols for _ in range(24)];r=c=0;scrolled=0;frames=[];anchors=[];cursor_visible=True
    def down():
        nonlocal r,grid,scrolled
        if r==23:grid.pop(0);grid.append([' ']*cols);scrolled+=1
        else:r+=1
    i=0
    while i<len(text):
        ch=text[i]
        if ch=='\x1b':
            m=CSI.match(text,i)
            if m:
                values=m[1].lstrip('?').split(';');n=int(values[0] or '0');n1=max(n,1);code=m[3]
                if code=='A':r=max(0,r-n1)
                elif code=='B':r=min(23,r+n1)
                elif code=='C':c=min(cols-1,c+n1)
                elif code=='D':c=max(0,c-n1)
                elif code in ['h','l'] and m[1]=='?25':cursor_visible=code=='h'
                elif code=='m' and n==0:
                    lines=[''.join(row) for row in grid]
                    if cols<31 and lines[r].startswith('◇ '):
                        assert grid[r][-1]==' ','compact footer used wrap column'
                        frames.append(lines);anchors.append(r)
                    elif cols>=31 and lines[r].strip() and set(lines[r].strip())=={'─'}:
                        height=8 if cols>=60 else 3
                        top=r-height+1
                        inset=2 if cols>=40 else 0
                        edge=min(cols-1,88)
                        if top>=0 and lines[top][inset:edge]=='─'*(edge-inset):
                            assert all(not lines[j][edge:].strip() for j in range(top,r+1)), 'panel used wrap column'
                            assert any('rusty /' in lines[j] for j in range(top+1,r)), 'status heading missing'
                            if height==8:
                                assert any(any('\u2801'<=ch<='\u28ff' for ch in lines[j]) for j in range(top+1,r)), 'knot missing'
                            frames.append(lines);anchors.append(r)
                elif code=='K':
                    if n==2:grid[r]=[' ']*cols
                    elif n==0:grid[r][min(c,cols):]=[' ']*max(0,cols-c)
                    elif n==1:grid[r][:c+1]=[' ']*(c+1)
                i=m.end();continue
        elif ch=='\r':c=0
        elif ch=='\n':down()
        elif ch=='\b':c=max(0,c-1)
        elif ord(ch)>=32 and ord(ch)!=127:
            if c>=cols:c=0;down()
            width=2 if unicodedata.east_asian_width(ch) in ['W','F'] else 1
            grid[r][c]=ch
            if width==2 and c+1<cols:grid[r][c+1]=''
            c+=width
        i+=1
    final=[''.join(row) for row in grid]
    assert not any('rusty /' in line or any('\u2801'<=ch<='\u28ff' for ch in line) for line in final),'transient panel left at exit'
    assert cursor_visible,'cursor was not restored at exit'
    assert 'CAPTURE_COMPLETE' in text
    assert frames, 'no complete footer frames captured'
    if frames:
        first=next((i for i,a in enumerate(anchors) if a==23),None)
        if require_scroll:
            assert scrolled>0
            assert first is not None
        if first is not None:
            assert all(a==23 for a in anchors[first:]), 'panel shifted after reaching bottom'
    return {'width':cols,'passed':True,'complete_frames':len(frames),'scrolls':scrolled,'footer_reached_bottom':23 in anchors,'clean_exit':True,'cursor_restored':cursor_visible},frames,final

if __name__=='__main__':
    root=Path(sys.argv[1]);reports=[];replays={}
    for width in [24,40,80,120]:
        result,frames,final=inspect(root/f'width-{width}/terminal-{width}.ansi',width)
        reports.append(result)
        replays[str(width)]=frames[::2] or [final]
        (root/f'width-{width}/last-screen.txt').write_text('\n'.join(final)+'\n')
    (root/'alignment.json').write_text(json.dumps({'passed':True,'checks':reports},indent=2)+'\n')
    (root/'replay.json').write_text(json.dumps(replays))
    print(json.dumps(reports))
