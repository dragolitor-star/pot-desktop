"""Measured source review of labels omitted by the initial crop reader.

Only source-confirmed controls are listed. The full local detector quad owns
each word, rather than its less reliable CTC per-character alignment.
"""
import json, math
from pathlib import Path
from PIL import Image

P=Path('tmp/pdfs/manual-live-full');D=P/'live-images'
lines=json.loads((P/'remaining-source-lines.json').read_text(encoding='utf-8'))
MAP={
 '936ac':{'车身角度偏':('车身角度偏','Gövde açısı sapması'),
          '车身整体侧向偏':('车身整体侧向偏','Gövdenin yanal sapması')},
 'a408':{'CU肤行机特央就举开别试':('CTU执行机构负载举升测试','CTU yük altında kaldırma testi')},
 '1212f5':{
  '年一住式标室皇程计':('叉车一体式标定（里程计、导航激光结合）','Forklift entegre kalibrasyonu (odometre ve navigasyon lazeri)'),
  '制银结束':('测试结束','Test bitti')},
 '82fa29':{
  '00三装米知下领头棋盈档':('0-0 工装采集下镜头棋盘格','0-0 Alt lens dama tahtası fikstür kaydı'),
  '00工装果集T设头棋型格':('0-0 工装采集下镜头棋盘格','0-0 Alt lens dama tahtası fikstür kaydı'),
  '信电动':('结束项','Bitiş öğesi')},
 'f1eb55':{
  '全注热':('任务监视器','Görev izleyici'),'口承体息':('能源信息','Enerji bilgileri'),
  '口国件信息':('固件信息','Aygıt yazılımı bilgileri'),'事共监':('事件监视器','Olay izleyici'),
  '全司':('告警监视器','Alarm izleyici'),'开利试':('开始测试','Testi başlat'),
  '起治克':('起始项','Başlangıç öğesi'),'活项':('结束项','Bitiş öğesi'),
  '0.2CYU执行第升零位标定':('0-2 CTU执行举升零位标定','0-2 CTU kaldırma sıfır kalibrasyonu')},
 '016505':{
  '式成坊':('测试成功','Test başarılı'), '测试洁重':('测试结束','Test bitti'),
  '式成动':('测试成功','Test başarılı'),'携试通过':('测试通过','Test başarılı'),
  '未别试':('未测试','Test edilmedi')},
 '1c448':{
  '专机有息':('整机信息','Makine bilgileri'),'三彩理信息':('能源信息','Enerji bilgileri'),
  '煎器件值息':('固件信息','Aygıt yazılımı bilgileri'),'中#警#训':('告警监视器','Alarm izleyici'),
  '中在务盟舰品':('任务监视器','Görev izleyici'),'试目':('测试组','Test grubu'),
  '4C清试':('4C2测试','4C2 testi'),'双百创试':('双目测距测试','Stereo mesafe testi'),
  '双目元情桥洲试':('双目无障碍测试','Stereo engelsiz test'),
  '测尚威功':('测试成功','Test başarılı'),
  '未激试':('未测试','Test edilmedi'),'未潮试':('未测试','Test edilmedi'),'未澳试':('未测试','Test edilmedi')},
 '49cee':{'4 C2测试':('4C2测试','4C2 testi')},
 '4b15f':{'未激试':('未测试','Test edilmedi')},
 '720aa':{'海里机售人':('海康机器人','HIKROBOT')},
 'a6684':{
  '日是车信':('基本信息','Temel bilgiler'),'营机信息':('整机信息','Makine bilgileri'),
  '自能系信息':('能源信息','Enerji bilgileri'),'件监阳器':('事件监视器','Olay izleyici'),
  '生产':('生产测试','Üretim testi'),'停利民验证先局':('序列号验证失败','Seri no doğrulanamadı'),
  '序号':('序号','Sıra no'),'确定':('确定','Tamam')},
 'd1572':{'原试精束':('测试结束','Test bitti'),'席列号险证失败':('序列号验证失败','Seri no doğrulanamadı'),
           '上传Meso':('上传Mes',"Mes'e yükle"),'时长(s)':('时长(s)','Süre (s)')},
 '44abee':{
  '设备买集额率：100:00顺/秒':('设备采集帧率：100.00帧/秒','Cihaz yakalama hızı: 100.00 kare/s'),
  '客户或手售视率：714领/秒':('客户端显示帧率：7.14帧/秒','İstemci görüntü hızı: 7.14 kare/s'),
  '图像接荐':('图像缓存','Görüntü önbelleği'),'序号':('序号','Sıra no'),
  '识别时间':('识别时间','Tanıma zamanı'),'码制':('码制','Kod türü'),'总体评':('总体评分','Toplam puan')},
}

def overlap(a,b):
 return max(0,min(a[2],b[2])-max(a[0],b[0]))*max(0,min(a[3],b[3])-max(a[1],b[1]))

def replace(regions,source,target,box,w,h):
 # A human-reviewed complete field replaces incomplete/duplicated readings.
 survivors=[]
 for r in regions:
  y0,x0,y1,x1=r['box_2d'];old=[x0*w/1000,y0*h/1000,x1*w/1000,y1*h/1000]
  if overlap(box,old)>.55*min((box[2]-box[0])*(box[3]-box[1]),(old[2]-old[0])*(old[3]-old[1])):continue
  survivors.append(r)
 x0,y0,x1,y1=box
 survivors.append(dict(source_text=source,translation=target,box_2d=[max(0,math.floor(y0*1000/h)),max(0,math.floor(x0*1000/w)),min(1000,math.ceil(y1*1000/h)),min(1000,math.ceil(x1*1000/w))],text_color_hex=None,bg_color_hex=None))
 return survivors

for image in lines:
 key=image['key'];prefix=next((p for p in MAP if key.startswith(p)),None)
 if prefix is None:continue
 path=D/(key+'.selected.regions.json');regions=json.loads(path.read_text(encoding='utf-8'));w,h=image['size'];changes=[]
 if prefix=='44abee':
  regions=[r for r in regions if r['source_text'] not in ['0X回0田口','属性树（100000020000000（00DA2279751)）']]
  regions=replace(regions,'属性树','Özellik ağacı',[21,24,60,39],w,h)
 for line in image['lines']:
  if line['ocr'] not in MAP[prefix]:continue
  source,target=MAP[prefix][line['ocr']];box=line['box']
  regions=replace(regions,source,target,box,w,h);changes.append(source)
 if prefix=='016505':
  regions=replace(regions,'临时修改车号，有效范围[60000-65000]范围，重启恢复配置0时，恢复成加密的ID',
   'Araç numarasını geçici olarak değiştirin. Geçerli aralık [60000-65000]. Yeniden başlatıp 0 yapılandırmasına dönünce şifreli ID geri yüklenir.',[101,572,239,620],w,h)
 if prefix=='1212f5':
  regions=replace(regions,'测试失败，客户端算法失败，AGV：正常，算法错误码：0，请重新测试',
   'Test başarısız. İstemci algoritması başarısız; AGV: normal; algoritma hata kodu: 0. Yeniden test edin.',[410,9,666,38],w,h)
 (D/(key+'.manual.regions.json')).write_text(json.dumps(regions,ensure_ascii=False,indent=2),encoding='utf-8')
 (D/(key+'.manual-review.json')).write_text(json.dumps(dict(method='Source bitmap and measured detector quad review',completed=changes),ensure_ascii=False,indent=2),encoding='utf-8')
 print(key,len(changes))
