"""Build source/result sheets for current measured-OCR previews and cached PNGs."""
import json
from pathlib import Path
from PIL import Image,ImageDraw
from review_image_fixtures import key_for,white_image

p=Path('tmp/pdfs/manual-live-full');d=p/'live-images'
provider=json.loads((p/'verification-provider.json').read_text(encoding='utf-8'))
glossary=json.loads((p/'image-glossary.json').read_text(encoding='utf-8'))
audit={r['key']:r for r in json.loads((p/'local-geometry-audit.json').read_text())} if (p/'local-geometry-audit.json').exists() else {}
entries=[]
for source in sorted(d.glob('*.source.png')):
 key=source.name.removesuffix('.source.png')
 with Image.open(source) as im:
  if key_for(im,provider,glossary)!=key:continue
 result=d/(key+'.png');status='native cache';r=audit.get(key)
 if r and not r['missing'] and not r['errors'] and (d/(key+'.preview.png')).exists():
  result=d/(key+'.preview.png');status='local preview'
 elif not result.exists():continue
 regions=d/(key+'.localized.regions.json') if status=='local preview' else d/(key+'.regions.json')
 entries.append({'key':key,'source':str(source.resolve()),'result':str(result.resolve()),'status':status,'regions':len(json.loads(regions.read_text(encoding='utf-8')))})
out=p/'geometry-review';out.mkdir(exist_ok=True)
(p/'geometry-review-index.json').write_text(json.dumps(entries,indent=2),encoding='utf-8')
for start in range(0,len(entries),6):
 sheet=Image.new('RGB',(1400,6*250),'#eeeeee');draw=ImageDraw.Draw(sheet)
 for row,e in enumerate(entries[start:start+6]):
  draw.text((8,row*250+3),f'{start+row+1} {e["key"][:14]} | {e["regions"]} labels | {e["status"]}',fill='black')
  for col,field in enumerate(['source','result']):
   im=white_image(e[field])
   if im.height<70:im=im.resize((im.width*3,im.height*3))
   im.thumbnail((680,225));sheet.paste(im,(col*700+8,row*250+22))
 sheet.save(out/f'sheet-{start//6+1:02}.png')
print(json.dumps({'readyForReview':len(entries),'sheets':(len(entries)+5)//6}))
