"""Verify the completed native export and render local visual-review sheets."""
import argparse
import json
from pathlib import Path
import re
import unicodedata

import fitz
from PIL import Image, ImageDraw


def normalize(text):
    return re.sub(r'\s', '', unicodedata.normalize('NFKC', text))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--directory', default='tmp/pdfs/manual-live-full')
    parser.add_argument('--pdf', required=True)
    parser.add_argument('--report', help='Explicit native fitting report (normally live-fit-report.json)')
    parser.add_argument('--diagnostic', action='store_true', help='Text-only diagnostic; never a final translated manual')
    args = parser.parse_args()
    directory = Path(args.directory)
    request = json.loads((directory/'layout-request.json').read_text(encoding='utf-8'))
    layouts = json.loads((directory/'canonical-layout.json').read_text(encoding='utf-8'))
    report = json.loads((Path(args.report) if args.report else directory/'live-fit-report.json').read_text(encoding='utf-8'))
    source = fitz.open(request['sourcePdfPath'])
    output = fitz.open(args.pdf)
    assert len(source) == len(output) == 137
    assert len(request['verificationPages']) == 137, 'A representative slice is not a final manual'
    assert not report['failures'], report['failures']
    fitted_ids = {block['blockId'] for block in report['fitted']}
    source_han = {b['blockId'] for page in layouts for b in page['blocks']
                  if re.search(r'[\u3400-\u9fff]', b['text'])}
    assert source_han <= fitted_ids, 'Source Chinese text was not replaced'
    missing_words = []
    residual = []
    invalid_glyph_pages = []
    graphics = []
    page_texts = []
    for index, (before, after) in enumerate(zip(source, output)):
        assert before.rect == after.rect, f'Page geometry changed: {index+1}'
        text = after.get_text()
        if any(marker in text for marker in ['\x00','\ufffd','CTRANSLATEUNI_']):
            invalid_glyph_pages.append(index+1)
        page_texts.append(normalize(text))
        if re.search(r'[\u3400-\u9fff]', text):
            residual.append(index+1)
        counts = {'page':index+1, 'sourcePaths':len(before.get_drawings()),
                  'outputPaths':len(after.get_drawings()), 'sourceImages':len(before.get_image_info()),
                  'outputImages':len(after.get_image_info())}
        assert counts['sourcePaths'] == counts['outputPaths'], counts
        assert counts['sourceImages'] == counts['outputImages'], counts
        graphics.append(counts)
    for block in report['fitted']:
        page = int(block['blockId'].split('_')[0][1:])-1
        if normalize(''.join(block['lines'])) not in page_texts[page]:
            missing_words.append(block['blockId'])
    result = {'kind':'native-text-diagnostic' if args.diagnostic else 'translated-manual',
              'pages':len(output), 'fittedBlocks':len(report['fitted']),
              'sourceChineseBlocksReplaced':len(source_han), 'residualChineseTextPages':residual,
              'invalidGlyphPages':invalid_glyph_pages,
              'unconfirmedTextExtraction':missing_words, 'graphics':graphics}
    (directory/('native-text-verification.json' if args.diagnostic else 'final-verification.json')).write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding='utf-8')
    assert not residual, residual
    assert not invalid_glyph_pages, invalid_glyph_pages
    assert not missing_words, missing_words

    if not args.diagnostic:
        checked = []
        fitted = {b['blockId']: b for b in report['fitted']}
        for number in [4,5,55,62,66,100,120,122]:
            block = next(b for b in layouts[number-1]['blocks'] if b['blockId']==f'p{number}_b2')
            text = ' '.join(fitted[block['blockId']]['lines'])
            box = block['bbox'];height=source[number-1].rect.height
            prefixes=[r['bbox'][2] for r in source[number-1].get_image_info()
                      if r['bbox'][2] <= box['left'] and box['left']-r['bbox'][2] < 35
                      and r['bbox'][1] < height-box['bottom'] and r['bbox'][3] > height-box['top']]
            assert prefixes, (number,'Missing source prefix')
            matches=[r for r in output[number-1].search_for(text)
                     if r.y0 < height-box['bottom'] and r.y1 > height-box['top']]
            assert matches and min(r.x0 for r in matches) >= max(prefixes)+6, (number,text,matches,prefixes)
            checked.append(number)
        result['chapterHeadingsNoBadgeOverlap']=checked
        (directory/'final-verification.json').write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding='utf-8')

    review = directory/('native-text-review' if args.diagnostic else 'review')
    review.mkdir(exist_ok=True)
    representatives = {1,2,3,4,5,6,8,17,27,29,30,41,55,62,66,69,74,77,80,81,82,91,100,103,113,120,122,127,129,132,137}
    sheet = None
    for index, page in enumerate(output):
        if index % 16 == 0:
            sheet = Image.new('RGB',(960,1440),'#eeeeee')
        pix = page.get_pixmap(matrix=fitz.Matrix(0.38,0.38),alpha=False)
        thumb = Image.frombytes('RGB',(pix.width,pix.height),pix.samples)
        x,y = (index % 4)*240,((index % 16)//4)*360
        ImageDraw.Draw(sheet).text((x+7,y+5),f'Page {index+1}',fill='black')
        sheet.paste(thumb,(x+5,y+25))
        if index % 16 == 15 or index == len(output)-1:
            sheet.save(review/f'contact-{index//16+1:02}.png')
        if index+1 in representatives:
            page.get_pixmap(matrix=fitz.Matrix(2,2),alpha=False).save(review/f'page-{index+1:03}.png')
    print(json.dumps({k:v for k,v in result.items() if k!='graphics'},ensure_ascii=False,indent=2))


if __name__ == '__main__':
    main()
