"""Replace eight native-rendered content/resource pairs, retaining page identities."""
import hashlib,json,shutil
from pathlib import Path
import fitz

root=Path('tmp/pdfs/manual-final');patch=Path('tmp/pdfs/manual-title-check')
pages=[4,5,55,62,66,100,120,122]
old=json.loads((root/'canonical-layout.json').read_text(encoding='utf-8'))
new=json.loads((patch/'canonical-layout.json').read_text(encoding='utf-8'))
changes=[]
for a,b in zip(old,new):
 assert len(a['blocks'])==len(b['blocks'])
 for x,y in zip(a['blocks'],b['blocks']):
  assert x['blockId']==y['blockId'] and x['text']==y['text']
  if x!=y:
   changes.append(x['blockId']);without=lambda r:{k:v for k,v in r.items() if k!='alignment'}
   assert without(x)==without(y),(x['blockId'],x,y)
   assert x['alignment']=='Center' and y['alignment']=='Left'
assert changes==[f'p{p}_b2' for p in pages],changes

base=fitz.open(root/'translated-final.pdf');fixed=fitz.open(patch/'title-diagnostic.pdf')
count=len(base);original_xrefs=[p.xref for p in base];toc=base.get_toc()
for n in pages:
 base.insert_pdf(fixed,from_page=n-1,to_page=n-1,start_at=-1)
 imported=base[-1].xref;original=original_xrefs[n-1]
 # Preserve page dictionaries, annotations and destinations. Only native
 # content and its matching resources change; imported refs are grafted locally.
 for key in ['Contents','Resources']:
  kind,value=base.xref_get_key(imported,key);assert kind!='null',(n,key)
  base.xref_set_key(original,key,value)
base.delete_pages(from_page=count,to_page=len(base)-1)
assert len(base)==count==137 and base.get_toc()==toc
out=root/'translated-ready.pdf';base.save(out,garbage=3,deflate=True);base.close();fixed.close()

before=fitz.open(root/'translated-final.pdf');after=fitz.open(out);unchanged=[]
for i in range(count):
 if i+1 in pages:continue
 a=before[i].get_pixmap(matrix=fitz.Matrix(.35,.35),alpha=False)
 b=after[i].get_pixmap(matrix=fitz.Matrix(.35,.35),alpha=False)
 assert a.samples==b.samples,f'Unrelated page changed: {i+1}'
 unchanged.append(i+1)

report=json.loads((root/'live-fit-report.json').read_text(encoding='utf-8'))
updates=json.loads((patch/'live-fit-report.json').read_text(encoding='utf-8'))
assert not report['failures'] and not updates['failures']
updated={r['blockId']:r for r in updates['fitted']}
report['fitted']=[updated.get(r['blockId'],r) for r in report['fitted']]
assert len(report['fitted'])==1495
shutil.copy2(root/'live-fit-report.json',root/'live-fit-report-before-title-review.json')
(root/'live-fit-report.json').write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
for target in [root,Path('tmp/pdfs/manual-live-full')]:shutil.copy2(patch/'canonical-layout.json',target/'canonical-layout.json')
proof=dict(nativeChapterPages=pages,layoutChanges=changes,unrelatedPagesPixelIdentical=unchanged,pages=count,outputSha256=hashlib.sha256(out.read_bytes()).hexdigest())
(root/'chapter-placement-verification.json').write_text(json.dumps(proof,indent=2),encoding='utf-8')
print(json.dumps(dict(chapterPages=len(pages),unchangedPages=len(unchanged),fittedBlocks=len(report['fitted']),bytes=out.stat().st_size)))
