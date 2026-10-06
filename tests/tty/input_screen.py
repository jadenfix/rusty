#!/usr/bin/env python3
"""Check busy composer caret ownership and clipping from real ANSI captures.
A small VT model; fixtures use ASCII drafts so font emoji width is not inferred.
"""
import json
from pathlib import Path
import re
import sys
import unicodedata
CSI = re.compile(r'\x1b\[([0-9;?]*)([ -/]*)([@-~])')

def inspect_screen(path, cols):
    grid=[[' ']*cols for _ in range(24)];r=c=0;frames=[];bottom_seen=False
    text=path.read_bytes().decode('utf-8');i=0
    def down():
        nonlocal r
        if r==23:grid.pop(0);grid.append([' ']*cols)
        else:r+=1
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
                elif code=='G':c=min(cols-1,n1-1)
                elif code in ['H','f']:
                    r=min(23,n1-1);c=min(cols-1,max(1,int(values[1] or '1'))-1 if len(values)>1 else 0)
                elif code=='K':
                    if n==2:grid[r]=[' ']*cols
                    elif n==0:grid[r][min(c,cols):]=[' ']*max(0,cols-c)
                    elif n==1:grid[r][:c+1]=[' ']*(c+1)
                elif code=='h' and m[1]=='?25':
                    lines=[''.join(row) for row in grid]
                    height=8 if cols>=60 else 3 if cols>=31 else 1
                    top=r-height
                    panel=lines[max(0,top):r]
                    if top>=0 and lines[r].startswith('› ') and any('rusty /' in row or row.startswith('◇ standard/') for row in panel):
                        assert all(not row[-1:].strip() for row in lines[top:r+1]),'input/panel wrote wrap column'
                        assert 2<=c<cols-1,('caret out of draft viewport',c)
                        if r==23:bottom_seen=True
                        if bottom_seen:assert r==23,'composer moved after filling the screen'
                        frames.append({'row':r,'column':c,'input':lines[r].rstrip(),'panel_top':top})
                i=m.end();continue
        elif ch=='\r':c=0
        elif ch=='\n':down()
        elif ch=='\b':c=max(0,c-1)
        elif ord(ch)>=32 and ord(ch)!=127:
            if unicodedata.combining(ch):i+=1;continue
            if c>=cols:c=0;down()
            width=2 if unicodedata.east_asian_width(ch) in ['W','F'] else 1
            grid[r][c]=ch
            if width==2 and c+1<cols:grid[r][c+1]=''
            c+=width
        i+=1
    assert frames,'no active composer frames'
    typed=[f for f in frames if f['input']=='› instan']
    assert typed,'draft was never visibly painted'
    assert all(f['column']==8 for f in typed),'caret drifted while draft text stayed the same'
    return {'width':cols,'passed':True,'frames':len(frames),'typed_frames':len(typed),'bottom_anchored':bottom_seen,'caret_column_for_instan':8}

if __name__=='__main__':
    root=Path(sys.argv[1]);checks=[inspect_screen(root/f'unsent-{cols}/terminal.ansi',cols) for cols in [24,40,80,120]]
    (root/'screen-checks.json').write_text(json.dumps({'passed':True,'checks':checks},indent=2)+'\n')
    print(json.dumps(checks))
