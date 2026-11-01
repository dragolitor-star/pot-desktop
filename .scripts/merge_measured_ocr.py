"""Merge independent local geometry observations without approving translations."""
import json,shutil,os
from pathlib import Path
from review_image_fixtures import current_entries
p=Path('tmp/pdfs/manual-live-full');d=p/'live-images';count=0
for e in current_entries(p):
 k=e['key'];base=d/(k+'.local-ocr-v2.json')
 if not base.exists():continue
 doc=json.loads(base.read_text(encoding='utf-8'));seen={json.dumps(line,sort_keys=True) for line in doc['lines']}
 for suffix in ['.recovery-ocr.json','.measured-ocr-v3.json','.template-ocr-v4.json']:
  f=d/(k+suffix)
  if not f.exists():continue
  more=json.loads(f.read_text(encoding='utf-8'))
  assert more['width']==doc['width'] and more['height']==doc['height']
  for line in more['lines']:
   signature=json.dumps(line,sort_keys=True)
   if signature not in seen:doc['lines'].append(line);seen.add(signature)
 original=d/(k+'.local-ocr-original.json')
 if not original.exists():shutil.copy2(base,original)
 swap=d/(k+'.ocr-merge-tmp.json');swap.write_text(json.dumps(doc,ensure_ascii=False),encoding='utf-8');os.replace(swap,base);count+=1
print('Merged local observations:',count)
