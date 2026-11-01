"""Propose OCR-measured rectangles for human review; never approve a cache.

Uncertain matches remain explicit. Proposals do not weaken the native matcher.
"""
import json,math
from pathlib import Path
from rapidfuzz.distance import Levenshtein

p=Path('tmp/pdfs/manual-live-full');d=p/'live-images'
def norm(s):return ''.join(c.lower() for c in s if c.isalnum())
def area(r):return max(0,r[2]-r[0])*max(0,r[3]-r[1])
def intersection(a,b):return max(0,min(a[2],b[2])-max(a[0],b[0]))*max(0,min(a[3],b[3])-max(a[1],b[1]))
def overlaps(a,b):return intersection(a,b)>.5*min(area(a),area(b))
def sim(a,b):return Levenshtein.normalized_similarity(a,b) if a and b else 0
report=json.loads((p/'local-geometry-audit.json').read_text());summary=[]
for item in report:
 if not item['missing']:continue
 k=item['key'];rs=json.loads((d/(k+'.localized.regions.json')).read_text(encoding='utf-8'));raw=json.loads((d/(k+'.regions.json')).read_text(encoding='utf-8'));raw=[r for r in raw if r['source_text'].strip()!=r['translation'].strip()]
 if len(raw)!=len(rs):continue
 obs=d/(k+'.local-ocr-v2.json')
 if not obs.exists():continue
 doc=json.loads(obs.read_text(encoding='utf-8'));w=doc['width'];h=doc['height'];candidates=[]
 for line in doc['lines']:
  words=[word for word in line['words'] if norm(word['text'])]
  # Short contiguous windows retain actual physical word ownership.
  for start in range(len(words)):
   text='';rect=[1000,1000,0,0]
   for word in words[start:start+32]:
    text+=norm(word['text']);b=word['bbox'];rect=[min(rect[0],b['y0']*1000/h),min(rect[1],b['x0']*1000/w),max(rect[2],b['y1']*1000/h),max(rect[3],b['x1']*1000/w)]
    if len(text)>250:break
    candidates.append((text,rect.copy()))
 used=[r['box_2d'] for i,r in enumerate(rs) if i not in item['missing']];proof=[];unresolved=[]
 for i in sorted(item['missing'],key=lambda i:-len(norm(rs[i]['source_text']))):
  source=norm(rs[i]['source_text']);hint=raw[i]['box_2d'];possible=[]
  if not source:unresolved.append(i);continue
  for text,box in candidates:
   if min(len(source),len(text))<max(len(source),len(text))*.5:continue
   center=(abs(hint[0]+hint[2]-box[0]-box[2])+abs(hint[1]+hint[3]-box[1]-box[3]))/2
   if center>=90 or (len(source)<4 and center>35):continue
   ts=sim(source,text);iou=intersection(hint,box)/max(1,area(hint)+area(box)-intersection(hint,box))
   if not ((ts>=.45 and center<90) or (iou>.55 and ts>=.2)):continue
   if any(overlaps(box,b) for b in used):continue
   score=ts*.65+min(1,iou)*.25+max(0,1-center/100)*.1
   possible.append((score,ts,iou,center,text,box))
  if not possible:unresolved.append(i);continue
  score,ts,iou,center,text,box=max(possible,key=lambda c:c[0]);box=[max(0,math.floor(box[0])),max(0,math.floor(box[1])),min(1000,math.ceil(box[2])),min(1000,math.ceil(box[3]))]
  rs[i]['box_2d']=box;used.append(box);proof.append(dict(index=i,source=rs[i]['source_text'],measuredTranscript=text,box=box,textScore=ts,intersectionOverUnion=iou,reviewed=False))
 (d/(k+'.proposal.regions.json')).write_text(json.dumps(rs,ensure_ascii=False,indent=2),encoding='utf-8')
 (d/(k+'.proposal-proof.json')).write_text(json.dumps(dict(proposed=proof,unresolved=sorted(unresolved),approved=False),ensure_ascii=False,indent=2),encoding='utf-8')
 summary.append(dict(key=k,proposed=len(proof),unresolved=sorted(unresolved)))
(p/'geometry-proposals.json').write_text(json.dumps(summary,indent=2),encoding='utf-8')
print(json.dumps(dict(images=len(summary),proposedLabels=sum(r['proposed'] for r in summary),fullyProposedImages=sum(not r['unresolved'] for r in summary))))
