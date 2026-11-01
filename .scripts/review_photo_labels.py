"""Private manual verification: preserve unreadable print; translate one readable label.

The user explicitly requested preserving blurry photo text. All coordinates are
measured on the original photo, and every changed pixel is checked against the
glyph mask/translation layer. This helper is not a production dependency.
"""
import hashlib
import json
import os
from pathlib import Path

import cv2
import numpy as np
from PIL import Image, ImageDraw, ImageFont


directory = Path('tmp/pdfs/manual-live-full/live-images')
carton = 'a7a8ca70034b62aadb660c73f02e50d6a644b530a6e6218e863fa954d4cde110'
fixture = '450537ba6e4f33390aa6e3311f0f8acfa2a3423ff25544bbd33a35b61528739b'
instruction = 'Bulanık fotoğraf yazılarını olduğu gibi koru (önerilen)'

for key in [carton, fixture]:
    source = directory / (key + '.source.png')
    original = Image.open(source).convert('RGBA')
    review = dict(sourceKey=key, sourcePngSha256=hashlib.sha256(source.read_bytes()).hexdigest(),
                  userInstruction=instruction, preserved='Unreliably legible photo print/handwriting',
                  translated=[])
    result = original.copy()
    if key == fixture:
        # The bottom line 工装1 is readable; the partly obscured upper line is
        # preserved. Perspective follows the sloping paper surface.
        quad = np.float32([[294, 529], [355, 552], [357, 585], [296, 562]])
        width, height = 183, 99
        plane = np.float32([[0, 0], [width - 1, 0], [width - 1, height - 1], [0, height - 1]])
        to_plane = cv2.getPerspectiveTransform(quad, plane)
        to_photo = cv2.getPerspectiveTransform(plane, quad)
        rgba = np.array(original)
        rectified = cv2.warpPerspective(rgba[:, :, :3], to_plane, (width, height))
        gray = cv2.cvtColor(rectified, cv2.COLOR_RGB2GRAY)
        threshold = float(np.quantile(gray, .75)) * .68
        ink = (gray < threshold).astype('uint8') * 255
        ink = cv2.dilate(ink, np.ones((3, 3), np.uint8))
        mask = cv2.warpPerspective(ink, to_photo, original.size, flags=cv2.INTER_NEAREST)
        mask = cv2.dilate(mask, np.ones((3, 3), np.uint8))
        cleaned = cv2.inpaint(rgba[:, :, :3], mask, 3, cv2.INPAINT_TELEA)
        layer = Image.new('RGBA', (width, height))
        draw = ImageDraw.Draw(layer)
        font_path = Path(os.environ['WINDIR']) / 'Fonts' / 'ARIALNB.TTF'
        text = 'Fikstür 1'
        for size in range(80, 11, -1):
            font = ImageFont.truetype(str(font_path), size)
            box = draw.textbbox((0, 0), text, font=font)
            if box[2] - box[0] <= width - 12 and box[3] - box[1] <= height - 16:
                break
        x = (width - box[2] + box[0]) / 2 - box[0]
        y = (height - box[3] + box[1]) / 2 - box[1]
        draw.text((x, y), text, font=font, fill=(22, 22, 22, 255))
        warped = cv2.warpPerspective(np.array(layer), to_photo, original.size)
        base = np.dstack([cleaned, rgba[:, :, 3]])
        result = Image.alpha_composite(Image.fromarray(base), Image.fromarray(warped))
        changed = np.any(np.array(result) != rgba, axis=2)
        allowed = (mask > 0) | (warped[:, :, 3] > 0)
        assert not np.any(changed & ~allowed), 'Pixels outside the label must remain exact'
        assert np.array_equal(np.array(result)[475:525, 275:360], rgba[475:525, 275:360])
        review['translated'].append(dict(source='工装1', target=text, quad=quad.tolist(),
                                         changedPixels=int(changed.sum()), method='glyph mask and perspective text'))
    result.save(directory / (key + '.photo-reviewed.png'))
    (directory / (key + '.photo-review.json')).write_text(json.dumps(review, ensure_ascii=False, indent=2), encoding='utf-8')
    print(key, len(review['translated']))

# Readable main product line on a third carton. The smaller company/contact
# print and the circular brand mark remain unchanged under the user's choice.
key = 'f90197829086e811d17a5de01e6ba648cfb43bb0e65d96404c733bd2a3b59ae4'
source = directory / (key + '.source.png')
original = Image.open(source).convert('RGBA')
rgba = np.array(original)
quad = np.float32([[458.9, 376], [460.8, 226], [474.6, 226.2], [472.6, 376.2]])
width, height = 600, 56
plane = np.float32([[0, 0], [width-1, 0], [width-1, height-1], [0, height-1]])
to_plane = cv2.getPerspectiveTransform(quad, plane)
to_photo = cv2.getPerspectiveTransform(plane, quad)
rectified = cv2.warpPerspective(rgba[:, :, :3], to_plane, (width, height))
gray = cv2.cvtColor(rectified, cv2.COLOR_RGB2GRAY)
ink = (gray < float(np.quantile(gray, .75))*.68).astype('uint8')*255
ink = cv2.dilate(ink, np.ones((3, 3), np.uint8))
mask = cv2.warpPerspective(ink, to_photo, original.size, flags=cv2.INTER_NEAREST)
mask = cv2.dilate(mask, np.ones((3, 3), np.uint8))
cleaned = cv2.inpaint(rgba[:, :, :3], mask, 3, cv2.INPAINT_TELEA)
layer = Image.new('RGBA', (width, height))
draw = ImageDraw.Draw(layer)
text = 'Optoelektronik bağlantı teknolojisi çözümleri'
font_path = Path(os.environ['WINDIR']) / 'Fonts' / 'ARIALNB.TTF'
for size in range(90, 11, -1):
    font = ImageFont.truetype(str(font_path), size)
    box = draw.textbbox((0, 0), text, font=font)
    if box[2]-box[0] <= width-8 and box[3]-box[1] <= height-8:
        break
core = rectified[gray < 65]
color = tuple(int(v) for v in np.median(core, axis=0)) if len(core) else (45, 35, 25)
draw.text(((width-box[2]+box[0])/2-box[0], (height-box[3]+box[1])/2-box[1]),
          text, font=font, fill=(*color, 255))
warped = cv2.warpPerspective(np.array(layer), to_photo, original.size)
result = Image.alpha_composite(Image.fromarray(np.dstack([cleaned, rgba[:, :, 3]])), Image.fromarray(warped))
changed = np.any(np.array(result) != rgba, axis=2)
assert not np.any(changed & ~((mask > 0) | (warped[:, :, 3] > 0)))
assert np.array_equal(np.array(result)[215:390, 510:540], rgba[215:390, 510:540])
assert np.array_equal(np.array(result)[410:475, 415:445], rgba[410:475, 415:445])
result.save(directory / (key + '.photo-reviewed.png'))
save_review = dict(sourceKey=key, sourcePngSha256=hashlib.sha256(source.read_bytes()).hexdigest(),
                   userInstruction=instruction, preserved='Smaller blurred company/contact print, English subtitle and brand mark',
                   translated=[dict(source='光电连接技术解决方案', target=text, quad=quad.tolist(),
                                    changedPixels=int(changed.sum()), method='measured perspective glyph mask')])
(directory / (key + '.photo-review.json')).write_text(json.dumps(save_review, ensure_ascii=False, indent=2), encoding='utf-8')
print(key, 1)
