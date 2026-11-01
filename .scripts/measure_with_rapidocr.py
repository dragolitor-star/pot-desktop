"""Offline second OCR opinion using installed models; no source transfer.

Only produces measured OCR data. It does not approve labels or author PNGs.
"""
import json,math,time
from pathlib import Path
import numpy as np
import onnxruntime
from PIL import Image
from rapidocr_onnxruntime import RapidOCR
from review_image_fixtures import current_entries

onnxruntime.disable_telemetry_events()
p=Path('tmp/pdfs/manual-live-full');d=p/'live-images'
engine=RapidOCR(intra_op_num_threads=2,inter_op_num_threads=2,
    max_side_len=6000,det_limit_side_len=2048,det_limit_type='max')
for entry in current_entries(p):
 k=entry['key'];output=d/(k+'.measured-ocr-v3.json')
 baseline=d/(k+'.local-ocr-v2.json')
 if entry['complete'] or output.exists() or not baseline.exists():continue
 im=Image.open(d/(k+'.source.png')).convert('RGBA')
 rgb=Image.alpha_composite(Image.new('RGBA',im.size,'white'),im).convert('RGB')
 scale=min(4,max(1,4000//max(im.size)));rgb=rgb.resize((im.width*scale,im.height*scale),Image.Resampling.LANCZOS)
 s=time.time();result,_=engine(np.array(rgb)[:,:,::-1],return_word_box=True)
 doc=json.loads(baseline.read_text(encoding='utf-8'))
 recovery=d/(k+'.recovery-ocr.json')
 if recovery.exists():doc=json.loads(recovery.read_text(encoding='utf-8'))
 sx=doc['width']/rgb.width;sy=doc['height']/rgb.height
 def bbox(quad):
  xs,ys=zip(*quad)
  return dict(x0=max(0,math.floor(min(xs)*sx)),y0=max(0,math.floor(min(ys)*sy)),
      x1=min(doc['width'],math.ceil(max(xs)*sx)),y1=min(doc['height'],math.ceil(max(ys)*sy)))
 added=0
 for item in result or []:
  quad,text,score,*rest=item
  if score<.6:continue
  # Axis aligned renderer cannot safely translate tilted labels in photos.
  if abs(quad[1][1]-quad[0][1])>max(3,abs(quad[1][0]-quad[0][0])*.15):continue
  if rest and len(rest)>=3:
   boxes,texts,scores=rest
   words=[dict(text=t,bbox=bbox(b)) for b,t in zip(boxes,texts)]
   doc['lines'].append(dict(words=words))
  doc['lines'].append(dict(words=[dict(text=text,bbox=bbox(quad))]));added+=1
 (d/(k+'.rapidocr-v3-raw.json')).write_text(json.dumps(dict(width=rgb.width,height=rgb.height,result=result),ensure_ascii=False),encoding='utf-8')
 output.write_text(json.dumps(doc,ensure_ascii=False),encoding='utf-8')
 print(k,added,round(time.time()-s,1),flush=True)
