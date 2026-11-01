"""Correct source-reviewed fragment mappings; retain source commands literally."""
import json
from pathlib import Path
updates={
 'p66_b10':'• Robot otomatik görev yaparken anormal biçimde dönüyor veya çalışma alanından çıkıyor mu, özellikle düz ya da kavisli hareket',
 'p66_b11':'sırasında gözlemleyin.',
 'p66_b12':'• Açık ve düz bir alanda net bir çizgiyi referans alarak uzaktan kumandayla test yapın. Robotun doğru hizada ilerleyip ilerleyemediğini',
 'p66_b13':'gözlemleyin. Sapma olursa ileri veya geri sıfır noktasını ayarlayın. Önce en ciddi sapmaya yol açan sıfır noktasını kalibre ederek robot hareketinin doğruluğunu ve kararlılığını sağlayın.',
 'p69_b8':'Set işlemi başarılı olduğunda "helm_the_calib set success" ibaresi görünür. Yeniden başlatınca mevcut kalibrasyon değerini kontrol edin. "cd',
 'p69_b9':'/home" komutunu girin ve "sz rcu_hw_cfg_out.json" ile json dosyasını indirin. İleri sıfır noktası 1018 ve geri sıfır noktası 1019 için sırasıyla alg_bias_pos ve alg_bias_neg alanlarını kontrol edin. Değer 0 ise veya alan yoksa kalibrasyon başarılı olmamıştır.',
 'p91_b3':'• Robotun paleti tanıyarak yük aldığı birkaç işlemi gözlemleyin. Çatalın sürekli aynı yönde sapması halinde (yani, tekrarlanan yük alma işlemlerinde',
 'p91_b4':'sapmanın aynı olması, örneğin paletin araca göre her defasında saat yönünün tersine kayması), O3X kamera/2D tanıma lazeri kalibrasyonunda sorun olduğu düşünülebilir.',
 'p91_b5':'• Bu durum ortaya çıkarsa bu kılavuzdaki manuel kalibrasyonu, terminal bağlantı aracına komut girerek uygulayabilirsiniz.',
 'p91_b6':'5.8.3 O3X kamera kalibrasyonu',
 'p91_b7':'• Önce açısal sapmayı, ardından yanal sapmayı ayarlayın.',
 'p91_b8':"• x sürümünde kalibrasyon değerlerinin birimleri mm ve 1/1000°'dir.",
 'p91_b9':'• Kalibrasyon değerlerini elle değiştirmeden önce orijinal değerleri mutlaka kaydedin.',
 'p91_b10':'Görüntüleme komutu: castor_fork_calib_set read_o3x',
 'p91_b11':'Yazma komutu: castor_fork_calib_set write_o3x x y yaw (örneğin castor_fork_calib_set',
 'p91_b12':'write_o3x 730 20 180000 )',
 'p91_b13':'Değişikliğin etkinleşmesi için reboot komutuyla yeniden başlatın.',
 'p91_b14':'5.8.4 2D lazer kalibrasyonu',
 'p91_b15':"Görüntüleme komutu: castor_calib_get_info laser. Çıktı Şekil 5-31'de gösterilmiştir.",
 'p91_b17':'castor_hw_show komutuyla koordinatlardan ilgili tanıma lazerini belirleyin. Örneğin 2D takip lazerinin gerçek koordinatları (x:845,y:-',
 'p91_b18':"65,z:62) ise, yalnızca uid 0X sonrasındaki değeri kopyalayıp onaltılık sistemden onluk sisteme dönüştürün. Sonuç, Şekil 5-32'deki uid değerine karşılık gelir.",
}
for name in ['manual-live-full','manual-final']:
 root=Path('tmp/pdfs')/name;path=root/'layout-request.json';backup=root/'layout-request-before-command-review.json'
 if not backup.exists():backup.write_bytes(path.read_bytes())
 request=json.loads(path.read_text(encoding='utf-8'));applied=[]
 for page in request['pages']:
  for bid,paragraph in zip(page['blockIds'],page['paragraphs']):
   if bid in updates:
    paragraph['translated']=updates[bid];applied.append(dict(blockId=bid,source=paragraph['original'],translation=paragraph['translated']))
 assert len(applied)==len(updates)
 path.write_text(json.dumps(request,ensure_ascii=False),encoding='utf-8')
 (root/'native-command-review.json').write_text(json.dumps(applied,ensure_ascii=False,indent=2),encoding='utf-8')
 translations=root/'verified-translations.json'
 if translations.exists():
  data=json.loads(translations.read_text(encoding='utf-8'))
  for a in applied:data[a['source']]=a['translation']
  translations.write_text(json.dumps(data,ensure_ascii=False,indent=2),encoding='utf-8')
 print(name,len(applied))
