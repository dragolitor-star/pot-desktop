"""Source-measured private raster repairs; glyph masks only, never rectangles.

Two difficult bitmaps have touching colored geometry or four-pixel table text.
These recipes keep every protected pixel outside measured ink/target glyphs.
Outputs require the same final visual review as native-rendered candidates.
"""
import hashlib
import json
from pathlib import Path

import cv2
import numpy as np
from PIL import Image, ImageDraw, ImageFont

D = Path('tmp/pdfs/manual-live-full/live-images')
FONT = 'C:/Windows/Fonts/ARIALN.TTF'

def paint(prefix, labels, base_suffix='.source.png'):
    source_path = next(D.glob(prefix + '*.source.png'))
    key = source_path.name.removesuffix('.source.png')
    source = Image.open(source_path).convert('RGBA')
    result = Image.open(D / (key + base_suffix)).convert('RGBA')
    original = np.array(source)
    allowed = np.zeros(original.shape[:2], np.uint8)
    records = []
    for source_text, target, box, mode, maximum in labels:
        x0,y0,x1,y1 = box
        rgb = original[y0:y1,x0:x1,:3].astype(int)
        colors,count=np.unique((rgb.reshape(-1,3)//8),axis=0,return_counts=True)
        bucket=colors[count.argmax()]
        flat_background=np.median(rgb[(rgb//8==bucket).all(2)],axis=0)
        backdrop = np.zeros_like(rgb)
        for y in range(y0,y1):
            for x in range(x0,x1):
                if prefix.startswith('4697'):
                    backdrop[y-y0,x-x0]=[251,229,214] if 150<=x<409 and 118<=y<424 else [255,255,255]
                elif 74<=y<302 and x>=372:
                    backdrop[y-y0,x-x0]=original[y,478,:3]
                else:
                    backdrop[y-y0,x-x0]=flat_background
        if mode == 'blue':
            ink = ((rgb[:,:,2] > rgb[:,:,0]+30) & (rgb[:,:,2] > rgb[:,:,1]+10)).astype('uint8')*255
        else:
            ink = ((np.max(backdrop-rgb,axis=2)>5) & (rgb.max(2)-rgb.min(2)<60)).astype('uint8')*255
            # Actual table rules run across a whole row, unlike glyph strokes.
            for row in range(len(ink)):
                if np.mean(ink[row]>0)>.99 and np.max(np.std(rgb[row],axis=0))<3:ink[row]=0
        assert ink.any(), (source_text,box)
        ink=cv2.dilate(ink,np.ones((3,3),np.uint8))
        mask=np.zeros(original.shape[:2],np.uint8);mask[y0:y1,x0:x1]=ink
        array=np.array(result)
        roi=array[y0:y1,x0:x1,:3];roi[ink>0]=backdrop[ink>0]
        result=Image.fromarray(array)
        allowed |= mask
        width,height=x1-x0,y1-y0
        chosen=None
        for size in np.arange(maximum,5.49,-.25):
            font=ImageFont.truetype(FONT,round(size*4));lines=['']
            for word in target.split():
                test=(lines[-1]+' '+word).strip()
                if font.getlength(test)>width*4 and lines[-1]:lines.append(word)
                else:lines[-1]=test
            bbox=font.getbbox('İŞĞgyp');line_height=bbox[3]-bbox[1]+2
            if max(font.getlength(line) for line in lines)<=width*4 and line_height*len(lines)<=height*4:
                chosen=(size,font,lines,line_height,bbox[1]);break
        assert chosen is not None, ('No complete fit',source_text,target,box)
        size,font,lines,line_height,top=chosen
        layer=Image.new('RGBA',(width*4,height*4));draw=ImageDraw.Draw(layer)
        y=max(0,(height*4-line_height*len(lines))//2)
        color=(91,155,213,255) if mode=='blue' else (55,55,55,255)
        for line in lines:
            draw.text((1,y-top),line,font=font,fill=color);y+=line_height
        layer=layer.resize((width,height),Image.Resampling.LANCZOS)
        result.alpha_composite(layer,(x0,y0))
        allowed[y0:y1,x0:x1] |= np.array(layer)[:,:,3]
        records.append(dict(source=source_text,translation=target,box=box,fontPixels=float(size),lines=lines))
    base=np.array(Image.open(D/(key+base_suffix)).convert('RGBA'))
    changed=np.any(np.array(result)!=base,axis=2)
    assert not np.any(changed & (allowed==0))
    output=D/(key+'.measured-reviewed.png');result.save(output)
    proof=dict(method='Measured source cells; glyph-only repair from the same source row/background; complete fitted Arial Narrow text',
               sourcePngSha256=hashlib.sha256(source_path.read_bytes()).hexdigest(),
               resultSha256=hashlib.sha256(output.read_bytes()).hexdigest(),
               baseSuffix=base_suffix,changedPixels=int(changed.sum()),regions=records,reviewed=False)
    (D/(key+'.measured-review.json')).write_text(json.dumps(proof,ensure_ascii=False,indent=2),encoding='utf-8')
    print(key,len(labels),flush=True)

paint('4697bb1f',[
    ('标定角度a','Kalibrasyon açısı a',(180,14,370,37),'dark',18),
    ('激光坐标系','Lazer koordinat sistemi',(111,120,217,143),'dark',14),
    ('标定值x','Kalibrasyon değeri x',(310,195,402,214),'blue',14),
    ('运动中心坐标系','Hareket merkezi koordinat sistemi',(97,238,260,259),'dark',14),
    ('标定值y','Kalibrasyon değeri y',(210,291,267,329),'blue',13),
])

labels=[
 ('标定 (10.20.138.35)','Kalibrasyon (10.20.138.35)',(9,6,146,23),'dark',9),
 ('标定项列表','Kalibrasyon öğeleri',(6,33,130,47),'dark',8),
 ('标定结果','Kalibrasyon sonuçları',(284,33,413,47),'dark',8),
 ('轮组编码器零位机械标定','Tekerlek enkoderi mekanik sıfır kalibrasyonu',(14,65,240,80),'dark',9),
 ('去标定','Kalibre et',(23,121,54,136),'dark',7),
 ('一体式标定（激光、相机、里程计）','Entegre kalibrasyon (lazer, kamera, odometre)',(14,192,245,209),'dark',9),
 ('去标定','Kalibre et',(23,250,54,264),'dark',7),
 ('标定过程信息','Kalibrasyon süreci bilgileri',(284,303,442,319),'dark',8),
 ('时间','Zaman',(291,57,361,72),'dark',7),
 ('标定项','Kalibrasyon',(371,57,474,72),'dark',7),
 ('耗时','Süre',(480,57,510,72),'dark',7),
 ('结果','Sonuç',(516,57,554,72),'dark',7),
 ('提示反馈','Geri bildirim',(561,57,680,72),'dark',7),
]
boundaries=[int(74+i*20.75) for i in range(12)]
rows=list(zip(boundaries,boundaries[1:]))
failures={0:'-4008',2:'-4008',3:'-4008',6:'-21',8:'0',9:'-4009',10:None}
for i,(top,bottom) in enumerate(rows):
    if i<10:
        labels.append(('一体式标定（激光、相机、里程计）','Entegre kalibrasyon (lazer, kamera, odometre)',(372,top,474,bottom),'dark',6))
    else:
        labels.append(('轮组编码器零位机械标定','Tekerlek enkoderi mekanik sıfır kalibrasyonu',(372,top,474,bottom),'dark',6))
    failed=i in failures
    labels.append(('未通过' if failed else '通过','Başarısız' if failed else 'Başarılı',(516,top,554,bottom),'dark',6))
    if i==10:continue
    if failed:
        code=failures[i]
        if i==8:
            source='测试失败，客户端向AGV发送文件失败，AGV:正常，算法错误码:0(正常)，请重新测试'
            target="Test başarısız. İstemci AGV'ye dosya gönderemedi. AGV: normal; algoritma hata kodu: 0 (normal). Yeniden test edin."
        else:
            source=f'测试失败，客户端算法库标定失败，AGV:正常，算法错误码:{code}(未知异常)，请重新测试'
            target=f'Test başarısız. İstemci algoritma kitaplığı kalibrasyonu başarısız. AGV: normal; algoritma hata kodu: {code} (bilinmeyen istisna). Yeniden test edin.'
    else:source,target='标定成功','Kalibrasyon başarılı'
    labels.append((source,target,(563,top,847,bottom),'dark',6))
paint('a2f21ac3',labels)
