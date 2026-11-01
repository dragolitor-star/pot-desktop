// Native opt-in verification counterpart of the Document window's local worker.
// Reads only the supplied PNG. No document pixels leave this machine.
const fs=require('node:fs');
const path=require('node:path');
const {createWorker}=require('tesseract.js');
(async()=>{
 const [input,output,width,height,language,psm,normalized]=process.argv.slice(2);
 const worker=await createWorker(language,1,{cachePath:path.resolve('tmp/pdfs/local-ocr-models'),langPath:'https://tessdata.projectnaptha.com/4.0.0'});
 try {
  await worker.setParameters({tessedit_pageseg_mode:psm,preserve_interword_spaces:'1'});
  const {data}=await worker.recognize(input);
  const lines=[...(data.lines||[])];
  if (normalized) {const {data:second}=await worker.recognize(normalized);lines.push(...(second.lines||[]));}
  fs.writeFileSync(output,JSON.stringify({width:Number(width),height:Number(height),lines:lines.map(line=>({words:(line.words||[]).map(word=>({text:word.text,bbox:word.bbox,confidence:word.confidence}))}))}));
 } finally {await worker.terminate();}
})().catch(error=>{console.error(String(error));process.exitCode=1;});
