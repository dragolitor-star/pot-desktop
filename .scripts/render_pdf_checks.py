"""Raster and extraction checks for the local diagnostic PDF, not a final translation."""
import json
from pathlib import Path
import fitz
from PIL import Image, ImageDraw

directory=Path('tmp/pdfs/manual-audit')
source=fitz.open(str(Path.home()/'OneDrive/Masaüstü/En detaylı kalibrasyon dökümanı.pdf'))
output=fitz.open(directory/'diagnostic_cached_layout.pdf')
report=json.loads((directory/'diagnostic_cached_layout.json').read_text(encoding='utf-8'))
assert len(source)==len(output)==137
checks=[]
for number in [1,2,6,27,41,74,77,80,81,82,103,127,132]:
    original=source[number-1]
    translated=output[number-1]
    assert original.rect==translated.rect
    images=[]
    for page in [original,translated]:
        pix=page.get_pixmap(matrix=fitz.Matrix(1.5,1.5),alpha=False)
        images.append(Image.frombytes('RGB',[pix.width,pix.height],pix.samples))
    sheet=Image.new('RGB',(images[0].width*2,images[0].height+30),'#eeeeee')
    draw=ImageDraw.Draw(sheet)
    draw.text((12,8),f'Page {number}: source',fill='black')
    draw.text((images[0].width+12,8),'Cached diagnostic (images and cache misses unchanged)',fill='black')
    for index,image in enumerate(images):
        sheet.paste(image,(index*image.width,30))
    sheet.save(directory/f'compare-p{number:03}.png')
    checks.append({'page':number,'source_paths':len(original.get_drawings()),'output_paths':len(translated.get_drawings()),'source_images':len(original.get_image_info()),'output_images':len(translated.get_image_info())})
for check in checks:
    assert check['source_paths']==check['output_paths'],check
    assert check['source_images']==check['output_images'],check
text=output[5].get_text()
print(json.dumps({'page_count':len(output),'fitted_blocks':len(report['fitted']),'fit_errors':len(report['failures']),'page6_tail_present':'önlenebilir' in text,'graphics':checks},ensure_ascii=False,indent=2))
