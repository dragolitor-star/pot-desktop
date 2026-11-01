// Offline geometry experiment: Tesseract rectangle coordinates are measured
// on the original OCR input, never taken from model-generated output boxes.
const fs=require('node:fs');
const path=require('node:path');
const {createWorker}=require('tesseract.js');
(async()=>{
 const jobs=JSON.parse(fs.readFileSync(process.argv[2],'utf8'));
 const worker=await createWorker('chi_sim',1,{cachePath:path.resolve('tmp/pdfs/local-ocr-models'),langPath:'https://tessdata.projectnaptha.com/4.0.0'});
 try {
  await worker.setParameters({tessedit_pageseg_mode:'6',preserve_interword_spaces:'1'});
  for(const job of jobs){
   if(fs.existsSync(job.output))continue;
   const document=JSON.parse(fs.readFileSync(job.baseline,'utf8'));
   for(const rectangle of job.rectangles){
    const {data}=await worker.recognize(job.input,{rectangle});
    document.lines.push(...(data.lines||[]));
   }
   fs.writeFileSync(job.output,JSON.stringify(document));
   console.log(path.basename(job.output),job.rectangles.length);
  }
 } finally {await worker.terminate();}
})().catch(error=>{console.error(String(error));process.exitCode=1;});
