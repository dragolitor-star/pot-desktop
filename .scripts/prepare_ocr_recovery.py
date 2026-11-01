import json
from pathlib import Path
from PIL import Image
from review_image_fixtures import current_entries

p=Path('tmp/pdfs/manual-live-full');d=p/'live-images';jobs=[]
for entry in current_entries(p):
 k=entry['key'];baseline=d/(k+'.local-ocr-v2.json');missing=d/(k+'.unmatched.json')
 if entry['complete'] or not baseline.exists() or not missing.exists():continue
 indices=json.loads(missing.read_text());regions=json.loads((d/(k+'.regions.json')).read_text(encoding='utf-8'))
 # The unmatched list indexes changed labels only.
 regions=[r for r in regions if r['source_text'].strip()!=r['translation'].strip()]
 if not indices or len(regions)>80 or entry['width']<250:continue
 w,h=Image.open(d/(k+'.local-ocr.png')).size;rectangles=[]
 for i in indices:
  if i>=len(regions):continue
  y0,x0,y1,x1=regions[i]['box_2d']
  dx=max(25/w*1000,(x1-x0)*.75);dy=max(15/h*1000,(y1-y0)*2)
  left=max(0,int((x0-dx)*w/1000));top=max(0,int((y0-dy)*h/1000))
  right=min(w,int((x1+dx)*w/1000));bottom=min(h,int((y1+dy)*h/1000))
  if bottom<=top or right<=left:continue
  rectangles.append(dict(left=left,top=top,width=right-left,height=bottom-top))
 if rectangles:jobs.append(dict(input=str((d/(k+'.local-ocr.png')).resolve()),baseline=str(baseline.resolve()),output=str((d/(k+'.recovery-ocr.json')).resolve()),rectangles=rectangles))
(p/'ocr-recovery-jobs.json').write_text(json.dumps(jobs,indent=2),encoding='utf-8')
print(json.dumps(dict(images=len(jobs),crops=sum(len(j['rectangles']) for j in jobs))))
