"""Find source-reviewed glyph patterns in repeated screenshots locally.

This creates OCR measurement candidates with pixel-match provenance, never a
translated bitmap. Only exact, independently reviewed source phrases teach it.
"""
import json,math,time
from pathlib import Path
import cv2,numpy as np
from PIL import Image
from review_image_fixtures import current_entries
def norm(s):return ''.join(c.lower() for c in s if c.isalnum())

p=Path('tmp/pdfs/manual-live-full');d=p/'live-images';library={}
for metadata in d.glob('*.manual.regions.json'):
 k=metadata.name.removesuffix('.manual.regions.json')
 if not (d/(k+'.manual-review.json')).exists():continue
 im=Image.open(d/(k+'.source.png')).convert('RGBA')
 im=Image.alpha_composite(Image.new('RGBA',im.size,'white'),im).convert('RGB');gray=cv2.cvtColor(np.array(im),cv2.COLOR_RGB2GRAY)
 for region in json.loads(metadata.read_text(encoding='utf-8')):
  # Screenshots and ordinary axis-aligned diagrams only.
  if region['source_text'].count('\n') or im.width<600:continue
  y0,x0,y1,x1=region['box_2d'];x0=math.floor(x0*im.width/1000);x1=math.ceil(x1*im.width/1000);y0=math.floor(y0*im.height/1000);y1=math.ceil(y1*im.height/1000)
  patch=gray[y0:y1,x0:x1]
  if patch.std()<20 or min(patch.shape)<6:continue
  library.setdefault(norm(region['source_text']),[]).append((patch,region['source_text'],k,(x0,y0,x1,y1)))

for entry in current_entries(p):
 k=entry['key'];out=d/(k+'.template-ocr-v4.json');baseline=d/(k+'.measured-ocr-v3.json')
 if entry['complete'] or out.exists():continue
 if not baseline.exists():baseline=d/(k+'.local-ocr-v2.json')
 if not baseline.exists():continue
 regions=entry['regions'];phrases={norm(r['source_text']) for r in regions if r['source_text']!=r['translation']}
 # A hamburger is a UI icon. Reconcile the known transcript error only when
 # the source-reviewed 菜单 glyph pattern is physically detected.
 if any('mapstudiopro' in s and '菜单' in s for s in phrases):phrases.add('菜单')
 if not phrases.intersection(library):continue
 im=Image.open(d/(k+'.source.png')).convert('RGBA');im=Image.alpha_composite(Image.new('RGBA',im.size,'white'),im).convert('RGB')
 gray=cv2.cvtColor(np.array(im),cv2.COLOR_RGB2GRAY);doc=json.loads(baseline.read_text(encoding='utf-8'));sx=doc['width']/im.width;sy=doc['height']/im.height;proof=[]
 for phrase in phrases.intersection(library):
  found=[]
  for patch,text,teacher,teacher_rect in library[phrase]:
   for scale in [.5,.667,.75,1,1.25,1.5,2,2.5,3]:
    tw=round(patch.shape[1]*scale);th=round(patch.shape[0]*scale)
    if th<6 or tw>im.width or th>im.height:continue
    template=cv2.resize(patch,(tw,th),interpolation=cv2.INTER_CUBIC)
    response=np.abs(cv2.matchTemplate(gray,template,cv2.TM_CCOEFF_NORMED))
    for _ in range(16):
     _,score,_,(x,y)=cv2.minMaxLoc(response)
     if score<.92:break
     response[max(0,y-th//2):min(response.shape[0],y+th//2+1),max(0,x-tw//2):min(response.shape[1],x+tw//2+1)]=0
     if any(abs(x-a)<tw/2 and abs(y-b)<th/2 for a,b in found):continue
     found.append((x,y));bbox=dict(x0=math.floor(x*sx),y0=math.floor(y*sy),x1=math.ceil((x+tw)*sx),y1=math.ceil((y+th)*sy))
     doc['lines'].append(dict(words=[dict(text=text,bbox=bbox)]))
     proof.append(dict(source=text,score=score,teacher=teacher,teacherRect=teacher_rect,measuredRect=(x,y,x+tw,y+th)))
 if proof:
  out.write_text(json.dumps(doc,ensure_ascii=False),encoding='utf-8')
  (d/(k+'.glyph-match-proof.json')).write_text(json.dumps(proof,ensure_ascii=False,indent=2),encoding='utf-8')
  print(k,len(proof),flush=True)
