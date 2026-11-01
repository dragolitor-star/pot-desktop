"""Source/result contact sheets with explicit current-build provenance checks."""
import hashlib
import json
from pathlib import Path
from PIL import Image, ImageDraw

root = Path('tmp/pdfs/manual-live-full')
directory = root / 'live-images'
exe = Path('src-tauri/target/debug/deps/pot-118ceaf5514dd672.exe')
entries = json.loads((root / 'selected-image-candidates.json').read_text(encoding='utf-8'))
review = root / 'selected-review'
review.mkdir(exist_ok=True)
def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()
def display(path):
    image = Image.open(path).convert('RGBA')
    return Image.alpha_composite(Image.new('RGBA', image.size, 'white'), image).convert('RGB')
for entry in entries:
    key = entry['key']
    source = directory / (key + '.source.png')
    entry['source'] = str(source.resolve())
    target = directory / (key + ('.photo-reviewed.png' if entry['kind']=='photo' else '.measured-reviewed.png' if entry['kind']=='measured' else '.selected-preview.png'))
    entry['status'] = 'pending'
    if entry['kind'] in ['photo','measured'] and target.exists():
        proof = json.loads((directory / (key + ('.photo-review.json' if entry['kind']=='photo' else '.measured-review.json'))).read_text(encoding='utf-8'))
        if proof['sourcePngSha256'] == digest(source):
            entry['status'] = entry['kind'] + ' source review'
    else:
        proof_path = directory / (key + '.selected-render-proof.json')
        if proof_path.exists() and target.exists():
            proof = json.loads(proof_path.read_text())
            current = (proof['sourceSha256']==digest(source)
                       and proof['regionsSha256']==digest(Path(entry['regions']))
                       and proof['previewSha256']==digest(target)
                       and (proof['rendererMtime']==exe.stat().st_mtime or
                            (proof.get('rendererArchive') and Path(proof['rendererArchive']).exists()
                             and proof.get('rendererSha256')==digest(Path(proof['rendererArchive'])))))
            entry['status'] = ('native pass' if proof['success'] else 'needs review') if current else 'stale'
    entry['result'] = str((target if target.exists() else source).resolve())
    entry['resultSha256'] = digest(target) if target.exists() else None
    entry['humanReviewed'] = False
for start in range(0,len(entries),6):
    sheet = Image.new('RGB',(1400,1500),'#eeeeee')
    draw = ImageDraw.Draw(sheet)
    for row,entry in enumerate(entries[start:start+6]):
        draw.text((8,row*250+3),f'{start+row+1} {entry["key"][:14]} | {entry["kind"]} | {entry["status"]}',fill='black')
        for col,field in enumerate(['source','result']):
            image = display(entry[field])
            if image.height < 70:
                image = image.resize((image.width*3,image.height*3))
            image.thumbnail((680,225))
            sheet.paste(image,(col*700+8,row*250+22))
    sheet.save(review / f'sheet-{start//6+1:02}.png')
(root/'selected-review-index.json').write_text(json.dumps(entries,ensure_ascii=False,indent=2),encoding='utf-8')
print(json.dumps(dict(images=len(entries),statuses={status:sum(e['status']==status for e in entries)
                                                 for status in sorted({e['status'] for e in entries})})))
