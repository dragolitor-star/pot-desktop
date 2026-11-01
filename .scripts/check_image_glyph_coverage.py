"""Flag source OCR Han glyphs that survived pixel-for-pixel in a result.

Diagnostic evidence only: photos/QR patterns can trigger OCR false positives.
"""
import argparse,json,math
from pathlib import Path
from PIL import Image
import numpy as np
from review_image_fixtures import current_entries

ap=argparse.ArgumentParser();ap.add_argument('--kind',default='preview');ap.add_argument('--prefix',action='append',default=[]);args=ap.parse_args()
p=Path('tmp/pdfs/manual-live-full');d=p/'live-images';audit={r['key']:r for r in json.loads((p/'local-geometry-audit.json').read_text())};flags=[];checked=0
selected={e['key']:e for e in json.loads((p/'selected-image-candidates.json').read_text(encoding='utf-8'))} if args.kind=='selected' else {}
for entry in current_entries(p):
 k=entry['key'];r=audit.get(k);raw=d/(k+'.rapidocr-v4-raw.json')
 if args.prefix and not any(k.startswith(prefix) for prefix in args.prefix):continue
 if not raw.exists():raw=d/(k+'.rapidocr-v3-raw.json')
 if not raw.exists():continue
 if args.kind=='preview' and (not r or r['missing'] or r['errors']):continue
 target=d/(k+'.'+('preview' if args.kind=='preview' else args.kind+'-preview')+'.png')
 if k in selected and selected[k]['kind'] in ['photo','measured']:target=Path(selected[k]['result'])
 if not target.exists():continue
 original=np.array(Image.open(d/(k+'.source.png')).convert('RGBA')).astype(int);result=np.array(Image.open(target).convert('RGBA')).astype(int)
 assert original.shape==result.shape
 h,w=original.shape[:2];data=json.loads(raw.read_text(encoding='utf-8'));sx=w/data['width'];sy=h/data['height'];unmodified=[]
 for item in data['result'] or []:
  if len(item)<6:continue
  _,_,_,boxes,texts,scores=item
  for quad,text,confidence in zip(boxes,texts,scores):
   if confidence<.8 or not any('\u3400'<=c<='\u9fff' for c in text):continue
   xs,ys=zip(*quad);x0=max(0,math.floor(min(xs)*sx));y0=max(0,math.floor(min(ys)*sy));x1=min(w,math.ceil(max(xs)*sx));y1=min(h,math.ceil(max(ys)*sy))
   if x0>=x1 or y0>=y1:continue
   changed=np.max(abs(original[y0:y1,x0:x1]-result[y0:y1,x0:x1]),axis=2)>10
   fraction=float(changed.mean())
   if fraction<.01:unmodified.append(dict(text=text,confidence=confidence,box=[x0,y0,x1,y1],changedFraction=fraction))
 checked+=1
 if unmodified:flags.append(dict(key=k,unchangedHanCandidates=unmodified))
output='image-glyph-coverage-flags.json' if args.kind=='preview' else args.kind+'-coverage-flags.json'
(p/output).write_text(json.dumps(flags,ensure_ascii=False,indent=2),encoding='utf-8')
print(json.dumps(dict(imagesChecked=checked,flaggedImages=len(flags),unchangedCandidates=sum(len(r['unchangedHanCandidates']) for r in flags))))
