"""Hybrid verification: match Vision text to local OCR geometry, never graph frames."""
import argparse
from collections import Counter
from difflib import SequenceMatcher
import json
from pathlib import Path
import re

from PIL import Image


def norm(text):
    return ''.join(re.findall(r'[\w\u3400-\u9fff]',text)).lower()


def match(source,box,lines,scale):
    x0,y0,x1,y1=box
    height=max(5,y1-y0)
    best=None
    for line in lines:
        words=[]
        for word in line.get('words',[]):
            b={k:v/scale for k,v in word['bbox'].items()}
            text=norm(word['text'])
            if not text or b['y1']-b['y0']>height*2:
                continue
            if abs((b['y0']+b['y1'])/2-(y0+y1)/2)>max(15,height*1.8):
                continue
            if b['x1']<x0-max(100,x1-x0) or b['x0']>x1+max(100,x1-x0):
                continue
            words.append((text,b))
        for start in range(len(words)):
            text=''
            selected=[]
            for end in range(start,min(len(words),start+len(source)+5)):
                text+=words[end][0]
                selected.append(words[end][1])
                if len(text)>len(source)*1.6+2:
                    break
                if len(text)<max(1,len(source)*.55):
                    continue
                ratio=SequenceMatcher(None,source,text).ratio()
                bounds=(min(b['x0'] for b in selected),min(b['y0'] for b in selected),max(b['x1'] for b in selected),max(b['y1'] for b in selected))
                dy=abs((bounds[1]+bounds[3]-y0-y1)/2)
                dx=abs((bounds[0]+bounds[2]-x0-x1)/2)
                score=ratio-.12*min(1,dy/max(1,height))-.08*min(1,dx/max(30,x1-x0))
                if best is None or score>best[0]:
                    best=(score,ratio,bounds,len(text)/max(1,len(source)))
    if best and best[1]>=.65 and best[0]>=.52:
        score,ratio,bounds,length=best
        if length<.85:
            bounds=(min(bounds[0],x0),bounds[1],max(bounds[2],x1),bounds[3])
        return bounds,ratio
    return None


def colors(image,box):
    x0,y0,x1,y1=[int(v) for v in box]
    pixels=list(image.crop((x0,y0,x1,y1)).convert('RGB').getdata())
    bucket=Counter(tuple(v//8 for v in p) for p in pixels).most_common(1)[0][0]
    cluster=[p for p in pixels if tuple(v//8 for v in p)==bucket]
    background=tuple(sorted(p[c] for p in cluster)[len(cluster)//2] for c in range(3))
    ink=sorted(pixels,key=lambda p:sum((p[c]-background[c])**2 for c in range(3)))
    foreground=ink[min(len(ink)-1,int(len(ink)*.97))]
    return foreground,background


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('source')
    parser.add_argument('ocr')
    parser.add_argument('--scale',type=float,default=3)
    parser.add_argument('--cropped-results')
    args=parser.parse_args()
    source=Path(args.source)
    prefix=source.name.removesuffix('.source.png')
    regions_file=source.with_name(prefix+'.regions.json')
    backup=source.with_name(prefix+'.vision.regions.json')
    if not backup.exists():
        backup.write_bytes(regions_file.read_bytes())
    regions=json.loads(backup.read_text(encoding='utf-8'))
    image=Image.open(source).convert('RGB')
    lines=json.loads(Path(args.ocr).read_text(encoding='utf-8'))['lines']
    cropped=json.loads(Path(args.cropped_results).read_text(encoding='utf-8')) if args.cropped_results else {}
    directory=source.parent/(prefix+'.ocr-patches')
    directory.mkdir(exist_ok=True)
    manifest=[]
    refined=[]
    unresolved=[]
    for index,region in enumerate(regions):
        y0,x0,y1,x1=region['box_2d']
        box=(x0*image.width/1000,y0*image.height/1000,x1*image.width/1000,y1*image.height/1000)
        found=match(norm(region['source_text']),box,lines,args.scale)
        if found is None and str(index) in cropped:
            value=cropped[str(index)]
            local=match(norm(region['source_text']),value['originalBox'],value['lines'],4)
            if local:
                b,score=local
                found=((b[0]+value['offset'][0],b[1]+value['offset'][1],b[2]+value['offset'][0],b[3]+value['offset'][1]),score)
        if found:
            b,score=found
            b=(max(0,int(b[0])-1),max(0,int(b[1])-1),min(image.width,int(b[2])+2),min(image.height,int(b[3])+2))
            region['box_2d']=[int(b[1]*1000/image.height),int(b[0]*1000/image.width),int((b[3]*1000+image.height-1)/image.height),int((b[2]*1000+image.width-1)/image.width)]
            foreground,background=colors(image,b)
            region['text_color_hex']='#' + ''.join(f'{v:02x}' for v in foreground)
            region['bg_color_hex']='#' + ''.join(f'{v:02x}' for v in background)
            refined.append({'index':index,'confidence':round(score,3),'box':b})
        else:
            unresolved.append(index)
            margin=max(48,(box[3]-box[1])*3)
            crop=(max(0,int(box[0]-margin)),max(0,int(box[1]-8)),min(image.width,int(box[2]+margin)),min(image.height,int(box[3]+8)))
            foreground,background=colors(image,box)
            patch=image.crop(crop)
            dark=sum(foreground)<sum(background)
            cutoff=sum(background)/3+(-18 if dark else 18)
            patch=patch.convert('L').point(lambda value:0 if (value<cutoff if dark else value>cutoff) else 255)
            patch=patch.resize((patch.width*4,patch.height*4),Image.Resampling.LANCZOS)
            path=directory/f'{index:03}.png'
            patch.save(path)
            manifest.append({'id':index,'path':str(path.resolve()),'offset':[crop[0],crop[1]],'originalBox':[box[0]-crop[0],box[1]-crop[1],box[2]-crop[0],box[3]-crop[1]]})
    (directory/'manifest.json').write_text(json.dumps(manifest),encoding='utf-8')
    source.with_name(prefix+'.ocr-refinement.json').write_text(json.dumps({'refined':refined,'unresolved':unresolved},ensure_ascii=False,indent=2),encoding='utf-8')
    if not unresolved:
        regions_file.write_text(json.dumps(regions,ensure_ascii=False,indent=2),encoding='utf-8')
    else:
        source.with_name(prefix+'.refined-preview.json').write_text(json.dumps(regions,ensure_ascii=False,indent=2),encoding='utf-8')
    print(json.dumps({'regions':len(regions),'matched':len(refined),'unresolved':unresolved,'manifest':str(directory/'manifest.json')}))


if __name__=='__main__':
    main()
