"""Small synthetic, non-private documents for local integration testing."""
from pathlib import Path
import json
import zipfile

root = Path(__file__).resolve().parents[1] / 'tmp' / 'web-fixtures'
root.mkdir(parents=True, exist_ok=True)
content = b'BT /F1 20 Tf 50 740 Td (Hello world) Tj ET'
objects = [
    b'<< /Type /Catalog /Pages 2 0 R >>',
    b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
    b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>',
    b'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>',
    b'<< /Length ' + str(len(content)).encode() + b' >>\nstream\n' + content + b'\nendstream',
]
pdf = bytearray(b'%PDF-1.4\n')
offsets = [0]
for index, obj in enumerate(objects, 1):
    offsets.append(len(pdf))
    pdf.extend(f'{index} 0 obj\n'.encode() + obj + b'\nendobj\n')
xref = len(pdf)
pdf.extend(f'xref\n0 {len(offsets)}\n0000000000 65535 f \n'.encode())
for offset in offsets[1:]:
    pdf.extend(f'{offset:010d} 00000 n \n'.encode())
pdf.extend(f'trailer\n<< /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n'.encode())
(root / 'sample.pdf').write_bytes(pdf)
with zipfile.ZipFile(root / 'sample.epub', 'w') as archive:
    archive.writestr('mimetype', 'application/epub+zip')
    archive.writestr('META-INF/container.xml', '<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>')
    archive.writestr('OEBPS/content.opf', '<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="book"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="book">web-test</dc:identifier><dc:title>Web Test</dc:title><dc:language>en</dc:language></metadata><manifest><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="chapter"/></spine></package>')
    archive.writestr('OEBPS/chapter.xhtml', '<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Test</title></head><body><p>Hello world</p><p>Keep every word.</p></body></html>')
try:
    from PIL import Image, ImageDraw, ImageFont
    image = Image.new('RGB', (600, 160), 'white')
    ImageDraw.Draw(image).text((35, 45), 'HELLO WORLD', fill='black', font=ImageFont.truetype('C:/Windows/Fonts/arial.ttf', 56))
    image.save(root / 'ocr.png')
except ImportError:
    pass
print(json.dumps({'fixtures': str(root)}))
