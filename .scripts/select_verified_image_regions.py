"""Prepare review candidates for every current bitmap; does not publish PNG caches."""
import json
import math
import re
from pathlib import Path

from PIL import Image
from manual_ui_lexicon import UI_LABELS
from review_image_fixtures import current_entries

root = Path('tmp/pdfs/manual-live-full')
directory = root / 'live-images'
def load(path):
    return json.loads(path.read_text(encoding='utf-8'))
def save(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2), encoding='utf-8')
def measured(key, labels, note):
    width, height = Image.open(directory / (key + '.source.png')).size
    regions = [dict(source_text=source, translation=target,
                    box_2d=[math.floor(y0*1000/height), math.floor(x0*1000/width),
                            math.ceil(y1*1000/height), math.ceil(x1*1000/width)],
                    text_color_hex=None, bg_color_hex=None)
               for source, target, (x0,y0,x1,y1) in labels]
    save(directory / (key + '.manual.regions.json'), regions)
    save(directory / (key + '.manual-review.json'), dict(method='Source bitmap pixel measurements', note=note))

# The circles are calibration fiducials, not Chinese. Only the red figure label
# is printed language. Its measured position excludes every marker and arrow.
measured('4875ad75b89f9162b1eb14dd430cd5a08b70e9c370c4e0fdb49ac580becd711d',
         [('图3', 'Şekil 3', (231,468,260,485))],
         'Read source at full size; preserve all circular markers and red arrows.')

console = '42fdb71ca378199dc2a1dea0f393f5faf4c1955d43a2af64de67b08d0ef7a5a2'
measured(console, [
    ('1.连上车', '1. Araca bağlan', (35,196,99,217)),
    ('2.点击shell', "2. shell'e tıkla", (259,47,339,68)),
    ('3.勾选客户端', '3. İstemciyi işaretle', (256,76,361,97)),
    ('4.在下面框中输入指令', '4. Alttaki kutuya komut gir', (436,388,637,410)),
], 'The large red instructions are readable. Console output is technical ASCII data, not Chinese; retain it exactly.')
save(directory / (console + '.legibility-review.json'), dict(
    userInstruction='Bulanık fotoğraf yazılarını olduğu gibi koru (önerilen)',
    preservedAreas=[[8,3,760,45],[9,68,196,195],[9,272,203,420]],
    protectedTechnicalArea=[208,94,764,383],
    reason='Tiny background UI details are blurred; false Han readings inside ASCII logs were rejected by source inspection.'))

# The exact clockwise question is also printed as PDF text on source page 110.
# Tiny background table glyphs in this 769x417 screenshot are genuinely blurred;
# do not fabricate those rows from another screenshot.
dialog = 'e6cddeb5d9dd2e860c3e62595ddb8970cb7760e69096d836c26202be9b9483d2'
measured(dialog, [
    ('提示', 'Bilgi', (223,106,238,117)),
    ('请查看一下当前托盘是否与AGV水平,是否需要顺时针调整?',
     'Paletin AGV ile hizasını kontrol edin. Saat yönünde ayar gerekiyor mu?', (52,137,267,150)),
    ('294秒后将自动取消', '294 saniye sonra otomatik iptal', (52,163,132,175)),
    ('取消', 'İptal', (349,204,366,215)),
    ('确定', 'Tamam', (384,204,401,215)),
    ('欢迎使用AGV集成工具软件', 'AGV entegrasyon aracına hoş geldiniz', (636,49,756,63)),
    ('海康机器人', 'HIKROBOT', (357,393,412,408)),
], 'Foreground question verified against source page 110 and the source bitmap; button action is confirmation.')
save(directory / (dialog + '.legibility-review.json'), dict(
    userInstruction='Bulanık fotoğraf yazılarını olduğu gibi koru (önerilen)',
    preservedAreas=[[0,64,769,95],[0,225,769,389]],
    reason='Dim background UI details have only a few blurred source pixels; retain them without invented readings.'))

# Independent strong-model reading corrected the blurry crop-only readings.
# Preserve complete mixed text/ASCII labels, including Mes and the seconds unit.
screen = '5fef243c09dd013be5ba82bbcfec6d3bb10e8ea9f83f56018ee8fb64cb416c03'
regions = load(directory / (screen + '.contextual-strong.regions.json'))
width, height = Image.open(directory / (screen + '.source.png')).size
for region in regions:
    if region['source_text'] == '上传':
        region.update(source_text='上传Mes', translation="Mes'e yükle",
                      box_2d=[math.floor(155*1000/height), math.floor(830*1000/width),
                              math.ceil(181*1000/height), math.ceil(894*1000/width)])
    if region['source_text'].startswith('时长'):
        region.update(source_text='时长(s)', translation='Süre (s)',
                      box_2d=[math.floor(228*1000/height), math.floor(585*1000/width),
                              math.ceil(261*1000/height), math.ceil(660*1000/width)])
save(directory / (screen + '.manual.regions.json'), regions)
save(directory / (screen + '.manual-review.json'), dict(
    method='Measured OCR coordinates, independent source reading and manual mixed-span review',
    corrected=['Complete 上传Mes label, excluding the cloud icon', 'Complete 时长(s) label including its unit']))

audit = {entry['key']:entry for entry in load(root / 'local-geometry-audit.json')}
index = []
for entry in current_entries(root):
    key = entry['key']
    kind = None
    if (directory / (key + '.measured-review.json')).exists():
        index.append(dict(key=key, kind='measured', regions=None,
                          result=str(directory / (key + '.measured-reviewed.png'))))
        continue
    if (directory / (key + '.photo-review.json')).exists():
        index.append(dict(key=key, kind='photo', regions=None,
                          result=str(directory / (key + '.photo-reviewed.png'))))
        continue
    for candidate_kind in ['manual', 'assisted']:
        candidate = directory / (key + '.' + candidate_kind + '.regions.json')
        if candidate.exists():
            kind = candidate_kind
            regions = load(candidate)
            break
    if kind is None:
        record = audit.get(key)
        if record and not record['missing'] and not record['errors']:
            kind = 'localized'
            regions = load(directory / (key + '.localized.regions.json'))
        else:
            index.append(dict(key=key, kind='pending', regions=None))
            continue
    for region in regions:
        source = ''.join(region['source_text'].split()).replace('（','(').replace('）',')')
        if source in UI_LABELS:
            region['translation'] = UI_LABELS[source]
        elif re.fullmatch(r'第\d+章', source):
            region['translation'] = 'Bölüm ' + source[1:-1]
        elif re.fullmatch(r'步骤\d+', source):
            region['translation'] = 'Adım ' + source[2:]
    # Independent OCR observations can describe the same physical field twice.
    # Merge only identical complete source/target strings with strong overlap;
    # neighboring rows and differing technical tokens remain independent.
    unique=[]
    for region in regions:
        a=region['box_2d'];duplicate=None
        for previous in unique:
            if ''.join(previous['source_text'].split())!=''.join(region['source_text'].split()) or previous['translation']!=region['translation']:continue
            b=previous['box_2d'];overlap=max(0,min(a[2],b[2])-max(a[0],b[0]))*max(0,min(a[3],b[3])-max(a[1],b[1]))
            if overlap>.6*min((a[2]-a[0])*(a[3]-a[1]),(b[2]-b[0])*(b[3]-b[1])):
                duplicate=previous;break
        if duplicate is None:unique.append(region)
        else:
            b=duplicate['box_2d'];duplicate['box_2d']=[min(a[0],b[0]),min(a[1],b[1]),max(a[2],b[2]),max(a[3],b[3])]
    regions=unique
    candidate = directory / (key + '.selected.regions.json')
    save(candidate, regions)
    index.append(dict(key=key, kind=kind, regions=str(candidate), labels=len(regions)))
save(root / 'selected-image-candidates.json', index)
print(json.dumps(dict(images=len(index), kinds={kind:sum(e['kind']==kind for e in index)
                                               for kind in sorted({e['kind'] for e in index})})))
