"""Index only current native-image fixtures and prepare source/result previews."""
import argparse
import hashlib
import json
from pathlib import Path
import struct

from PIL import Image, ImageDraw


def key_for(image, provider, glossary, source_lang='zh_cn', cache_version=b'image-glyph-inpaint-v5-local-ocr'):
    digest = hashlib.sha256()
    for value in [cache_version,struct.pack('<I',image.width),
                  struct.pack('<I',image.height),image.convert('RGBA').tobytes(),
                  source_lang.encode(),b'tr',provider.get('visionModel',provider['model']).encode(),
                  provider.get('visionBase',provider['base']).encode()]:
        digest.update(struct.pack('<Q',len(value)))
        digest.update(value)
    for entry in glossary:
        digest.update(entry['sourceTerm'].encode()+b'\0'+entry['targetTerm'].encode()+b'\0')
    return digest.hexdigest()


def current_entries(directory):
    provider = json.loads((directory/'verification-provider.json').read_text(encoding='utf-8'))
    glossary = json.loads((directory/'image-glossary.json').read_text(encoding='utf-8'))
    entries = []
    used_file = directory/'live-images/used-keys.json'
    used = set(json.loads(used_file.read_text())) if used_file.exists() else None
    for path in sorted((directory/'live-images').glob('*.source.png')):
        key = path.name.removesuffix('.source.png')
        if used is not None and key not in used:
            continue
        with Image.open(path) as source:
            if key_for(source,provider,glossary) != key:
                continue
            result = path.with_name(key+'.png')
            regions_file = path.with_name(key+'.regions.json')
            entries.append({'key':key,'width':source.width,'height':source.height,
                            'complete':result.exists(),'regions':json.loads(regions_file.read_text(encoding='utf-8')) if regions_file.exists() else []})
    return entries


def white_image(path):
    image = Image.open(path).convert('RGBA')
    return Image.alpha_composite(Image.new('RGBA',image.size,'white'),image).convert('RGB')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--directory',default='tmp/pdfs/manual-live-full')
    args = parser.parse_args()
    directory = Path(args.directory)
    entries = current_entries(directory)
    review = directory/'image-review'
    review.mkdir(exist_ok=True)
    (directory/'current-image-index.json').write_text(json.dumps(entries,ensure_ascii=False,indent=2),encoding='utf-8')
    completed = [entry for entry in entries if entry['complete']]
    for start in range(0,len(completed),8):
        sheet = Image.new('RGB',(1200,8*220),'#eeeeee')
        draw = ImageDraw.Draw(sheet)
        for row,entry in enumerate(completed[start:start+8]):
            key = entry['key']
            draw.text((10,row*220+3),f'{start+row+1}: {key[:12]} ({len(entry["regions"])} regions)',fill='black')
            for col,suffix in enumerate(['.source.png','.png']):
                image = white_image(directory/'live-images'/(key+suffix))
                if image.height<80:
                    image = image.resize((image.width*3,image.height*3))
                image.thumbnail((580,195))
                sheet.paste(image,(col*600+10,row*220+22))
        sheet.save(review/f'images-{start//8+1:02}.png')
    print(json.dumps({'current_images':len(entries),'completed':len(completed),
                      'regions':sum(len(entry['regions']) for entry in completed)}))


if __name__ == '__main__':
    main()
