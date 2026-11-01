"""Measure recognition gains only; never author a translated bitmap/cache."""
import json
from pathlib import Path
from rapidfuzz.distance import Levenshtein

def norm(s):return ''.join(c.lower() for c in s if c.isalnum())
def candidates(doc):
 out=set()
 for line in doc['lines']:
  words=[norm(w['text']) for w in line['words'] if norm(w['text'])]
  for start in range(len(words)):
   text=''
   for word in words[start:start+48]:text+=word;out.add(text)
 return out
def matches(source,cs):
 s=norm(source)
 return any(min(len(s),len(t))*4>=max(len(s),len(t))*3 and Levenshtein.normalized_similarity(s,t)>=.7 for t in cs)
p=Path('tmp/pdfs/manual-live-full/live-images');report=[]
for recovery in p.glob('*.recovery-ocr.json'):
 k=recovery.name.removesuffix('.recovery-ocr.json');original=p/(k+'.local-ocr-before-recovery.json')
 if not original.exists():original=p/(k+'.local-ocr-v2.json')
 a=candidates(json.loads(original.read_text(encoding='utf-8')));b=candidates(json.loads(recovery.read_text(encoding='utf-8')))
 regions=json.loads((p/(k+'.regions.json')).read_text(encoding='utf-8'))
 labels=[r['source_text'] for r in regions if r['source_text'].strip()!=r['translation'].strip()]
 report.append(dict(key=k,labels=len(labels),before=sum(matches(s,a) for s in labels),after=sum(matches(s,b) for s in labels)))
Path('tmp/pdfs/manual-live-full/ocr-recovery-gain.json').write_text(json.dumps(report,indent=2))
print(json.dumps(dict(images=len(report),labels=sum(r['labels'] for r in report),matchedBefore=sum(r['before'] for r in report),matchedAfter=sum(r['after'] for r in report))))
