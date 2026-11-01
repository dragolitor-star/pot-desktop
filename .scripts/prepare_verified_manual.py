"""Package only visually reviewed current source/result hashes for a local export."""
import collections,hashlib,json,re,shutil
from pathlib import Path
from PIL import Image
from review_image_fixtures import key_for

root=Path('tmp/pdfs/manual-live-full');final=Path('tmp/pdfs/manual-final');images=final/'live-images'
images.mkdir(parents=True,exist_ok=True)
entries=json.loads((root/'selected-review-index.json').read_text(encoding='utf-8'))
initial=json.loads((root/'initial-contact-review.json').read_text(encoding='utf-8'))
assert len(entries)==len({e['key'] for e in entries})==270
digest=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()
tokens=re.compile(r'(?:[A-Za-z0-9_.-]+[/\\])+[A-Za-z0-9_.-]+|[A-Za-z][A-Za-z0-9_.-]*\.(?:txt|bin|json|xml|ini|cfg|log|csv|dll|exe)|[A-Za-z][A-Za-z0-9_]*_[A-Za-z0-9_]+|0[xX](?:[0-9A-Fa-f]+)?|[A-Z]{2,}[0-9]*|-?\d+(?:\.\d+)*')
approval=[];token_failures=[]
for e in entries:
 assert e['status'] in ['native pass','measured source review','photo source review'],e
 assert digest(e['result'])==e['resultSha256'],e['key']
 with Image.open(e['source']) as a,Image.open(e['result']) as b:assert a.size==b.size,e['key']
 if e['regions']:
  for r in json.loads(Path(e['regions']).read_text(encoding='utf-8')):
   missing=collections.Counter(tokens.findall(r['source_text']))-collections.Counter(tokens.findall(r['translation']))
   if missing:token_failures.append(dict(key=e['key'],source=r['source_text'],translation=r['translation'],missing=list(missing.elements())))
 approval.append(dict(key=e['key'],source=e['source'],sourceSha256=digest(e['source']),result=e['result'],resultSha256=digest(e['result']),kind=e['kind'],reviewed=True,reviewer='Codex visual source/result review',evidence='45 initial source/result contact sheets; 12 changed-result contact sheets; full-size and critical label crops',changedSinceInitialReview=initial[e['key']]['resultSha256']!=e['resultSha256']))
(root/'protected-image-token-audit.json').write_text(json.dumps(token_failures,ensure_ascii=False,indent=2),encoding='utf-8')
assert not token_failures,token_failures
(root/'image-visual-approval.json').write_text(json.dumps(approval,ensure_ascii=False,indent=2),encoding='utf-8')
for name in ['layout-request.json','canonical-layout.json','verification-provider.json','image-glossary.json']:
 shutil.copy2(root/name,final/name)
provider=json.loads((final/'verification-provider.json').read_text(encoding='utf-8'));glossary=json.loads((final/'image-glossary.json').read_text(encoding='utf-8'))
assert not any('key' in k.lower() for k in provider), 'Provider manifest must not contain credentials'
mapping=[]
for a in approval:
 with Image.open(a['source']) as source:key=key_for(source,provider,glossary,cache_version=b'image-glyph-inpaint-v6-measured-rows')
 assert digest(a['result'])==a['resultSha256']
 shutil.copy2(a['result'],images/(key+'.png'))
 mapping.append(dict(key=key,reviewedSourceKey=a['key'],sourceSha256=a['sourceSha256'],resultSha256=a['resultSha256'],kind=a['kind']))
assert len(mapping)==len({m['key'] for m in mapping})==270
(final/'approved-image-cache.json').write_text(json.dumps(mapping,ensure_ascii=False,indent=2),encoding='utf-8')
print(json.dumps(dict(approvedImages=len(mapping),kinds=dict(collections.Counter(a['kind'] for a in approval)),changedImages=sum(a['changedSinceInitialReview'] for a in approval),protectedTokenFailures=len(token_failures))))
