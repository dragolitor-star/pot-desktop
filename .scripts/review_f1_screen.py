"""Correct the photographed CTU screen from the actual source, not corrupt OCR."""
import json,math
from pathlib import Path
from PIL import Image

D=Path('tmp/pdfs/manual-live-full/live-images')
path=next(D.glob('f1eb55*.manual.regions.json'));key=path.name.split('.')[0]
regions=json.loads(path.read_text(encoding='utf-8'));w,h=Image.open(D/(key+'.source.png')).size
aliases={
 'OPO标示':('0 PO预标定','0 PO ön kalibrasyonu'),
 '中福':('上传Mes',"Mes'e yükle"),
 '新财诚装果':('查看测试结果','Test sonuçlarını göster'),
 '校前产品字外件':('校验产品序列号','Seri numarasını doğrula'),
 '提示信单':('提示信息','Durum bilgisi'),'时长':('时长(s)','Süre (s)'),
 '热甲':('结果','Sonuç'),'到优用训':('测试失败','Test başarısız'),
 '精体品型':('测试结束','Test bitti'),
 '举丹帆构归测诚正求':('举升机构归零测试正常','Kaldırma mekanizması sıfırlama testi normal'),
 '资试成动':('测试成功','Test başarılı'),'重界底':('测试结束','Test bitti'),
 'CTU抗行华升零包桥定':('CTU执行举升零位标定','CTU kaldırma sıfır kalibrasyonu'),
 'CTU状重电利试\n密试':('CTU载重电机测试','CTU yük motoru testi'),
}
regions=[r for r in regions if r['source_text'] not in ['车','点#星开家可迎对零点']]
for r in regions:
 if r['source_text'] in aliases:r['source_text'],r['translation']=aliases[r['source_text']]
 if r['source_text']=='未':
  r['source_text']='未测试';r['translation']='Test edilmedi'
  r['box_2d'][1]=math.floor(233*1000/w);r['box_2d'][3]=math.ceil(275*1000/w)

def field(source,target,box):
 global regions
 x0,y0,x1,y1=box
 def hit(r):
  a,b,c,d=r['box_2d'];a,b,c,d=b*w/1000,a*h/1000,d*w/1000,c*h/1000
  inter=max(0,min(c,x1)-max(a,x0))*max(0,min(d,y1)-max(b,y0))
  return inter>.5*min((c-a)*(d-b),(x1-x0)*(y1-y0))
 regions=[r for r in regions if not hit(r)]
 regions.append(dict(source_text=source,translation=target,box_2d=[math.floor(y0*1000/h),math.floor(x0*1000/w),math.ceil(y1*1000/h),math.ceil(x1*1000/w)],text_color_hex=None,bg_color_hex=None))

for source,target,box in [
 ('测试组','Test grubu',(21,72,56,89)),
 ('基本信息','Temel bilgiler',(27,39,74,55)),
 ('整机信息','Makine bilgileri',(94,39,141,55)),
 ('能源信息','Enerji bilgileri',(176,39,216,55)),
 ('固件信息','Aygıt yazılımı bilgileri',(246,38,283,54)),
 ('事件监视器','Olay izleyici',(313,37,356,53)),
 ('告警监视器','Alarm izleyici',(386,36,430,52)),
 ('任务监视器','Görev izleyici',(460,34,504,51)),
 ('生产测试','Üretim testi',(535,31,578,48)),
 ('0-2 CTU执行举升零位标定','0-2 CTU kaldırma sıfır kalibrasyonu',(420,68,522,83)),
 ('状态信息','Durum bilgisi',(694,141,736,156)),
 ('当前举升高度（相对零位）：0','Kaldırma yüksekliği (sıfıra göre): 0',(709,165,806,180)),
 ('举升零位高度：290','Sıfır kaldırma yüksekliği: 290',(709,182,804,197)),
]:field(source,target,box)
path.write_text(json.dumps(regions,ensure_ascii=False,indent=2),encoding='utf-8')
(D/(key+'.manual-review.json')).write_text(json.dumps(dict(method='Full photographed source at 2x and measured physical controls',corrected=list(aliases),note='Restore actual robot UI names; reject meaningless crop-reader interpretations.'),ensure_ascii=False,indent=2),encoding='utf-8')
print(key,len(regions))
