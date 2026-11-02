#!/usr/bin/env python3
"""M2-R1: execute pinned v0.1.0 binary on trusted local fixtures (NOT arbitrary-web compatibility)."""
import collections, hashlib, json, pathlib, subprocess, time

ROOT=pathlib.Path(__file__).resolve().parent
BIN=ROOT/"release/toy-browser-engine-v0.1.0-research-preview-x86_64-unknown-linux-gnu/browser_engine"
OUT=ROOT/"baseline-v0.1.0"
PAGES=OUT/"pages"
CAP=OUT/"captures"

def h(body,head=""):
    return '<!doctype html><html><head><meta charset="utf-8">'+head+'</head><body>'+body+'</body></html>'
def js(src,body='<div id="out">PENDING</div>'):
    return h(body+"<script>"+src+"</script>")
def style(css,body='<div class="box"></div>'):
    return h(body,'<style>html,body{margin:0;padding:0}'+css+'</style>')
def t(id,area,html,dom=(),pixels=()):
    return dict(id=id,area=area,html=html,dom=list(dom),pixels=list(pixels))
cases=[
t("H01","HTML",h('<h1>H01_HEADING</h1><p>H01_PARAGRAPH</p>'),("H01_HEADING","H01_PARAGRAPH")),
t("H02","HTML",h('<p>Tom &amp; Jerry &lt;3</p>'),("Tom &amp; Jerry","&lt;3")),
t("H03","HTML",js('document.getElementById("out").textContent = document.querySelectorAll("li").length;','<ul><li>A<li>B<li>C</ul><div id="out">PENDING</div>'),('id="out">3</div>',)),
t("H04","HTML",js('document.getElementById("out").textContent = document.querySelectorAll("p").length;','<div><p>first</p><p>second</p></div><div id="out">PENDING</div>'),('id="out">2</div>',)),
t("H05","HTML",h('<input id="name" value="guest"><label for="name">Name</label>'),('value="guest"','for="name"')),
t("C01","CSS/paint",style('html{background:#123456}'),pixels=((0,0,(18,52,86)),)),
t("C02","CSS/paint",style('.box{position:absolute;left:0;top:0;width:30px;height:30px;background:#1a2b3c}'),pixels=((5,5,(26,43,60)),)),
t("C03","CSS/paint",style('.row{display:flex}.a,.b{width:20px;height:20px;flex-shrink:0}.a{background:#ff0000}.b{background:#00ff00}','<div class="row"><div class="a"></div><div class="b"></div></div>'),pixels=((5,5,(255,0,0)),(25,5,(0,255,0)))),
t("C04","CSS/paint",style('.grid{display:grid;grid-template-columns:20px 20px}.a,.b{height:20px}.a{background:#ff0000}.b{background:#00ff00}','<div class="grid"><div class="a"></div><div class="b"></div></div>'),pixels=((5,5,(255,0,0)),(25,5,(0,255,0)))),
t("C05","CSS/paint",style('.a,.b{position:absolute;left:0;top:0;width:30px;height:30px}.a{background:red;z-index:1}.b{background:blue;z-index:2}','<div class="a"></div><div class="b"></div>'),pixels=((5,5,(0,0,255)),)),
t("C06","CSS/paint",style(':root{--accent:#0066cc}.box{position:absolute;left:0;top:0;width:30px;height:30px;background:var(--accent)}'),pixels=((5,5,(0,102,204)),)),
t("C07","CSS/paint",style('.box{position:absolute;left:0;top:0;width:30px;height:30px;background:red}@media (min-width:100px){.box{background:#00aa00}}'),pixels=((5,5,(0,170,0)),)),
t("C08","CSS/paint",style('.box{position:absolute;left:0;top:0;width:30px;height:30px;background:#00ff00;border-radius:15px}'),pixels=((0,0,(255,255,255)),(15,15,(0,255,0)))),
t("I01","images",h('<img src="tiny.ppm" width="30" height="30">','<style>html,body{margin:0;padding:0}</style>'),pixels=((5,5,(200,10,30)),)),
t("J01","JS/DOM",js('document.getElementById("out").textContent="J01_DONE";'),('id="out">J01_DONE</div>',)),
t("J02","JS/DOM",js('document.getElementById("out").innerHTML="<strong>J02_DONE</strong>";'),('<strong>J02_DONE</strong>',)),
t("J03","JS/DOM",js('let p=document.createElement("p"); p.textContent="J03_DONE"; document.getElementById("out").appendChild(p);'),('<p>J03_DONE</p>',)),
t("J04","JS/DOM",js('document.getElementById("out").textContent=document.querySelectorAll(".item").length;','<div class="item"></div><div class="item"></div><div id="out">PENDING</div>'),('id="out">2</div>',)),
t("J05","JS/DOM",js('let x=0;function inc(){x=x+1;}inc();inc();document.getElementById("out").textContent=x;'),('id="out">2</div>',)),
t("J06","JS/DOM",js('let a=[1,2,3];document.getElementById("out").textContent=a.map(x=>x*2).join(",");'),('id="out">2,4,6</div>',)),
t("J07","JS/DOM",js('document.getElementById("out").textContent="UNICODE_中文字😀";'),('UNICODE_中文字😀',)),
t("J08","JS/DOM",js('document.getElementById("out").classList.add("active");'),('class="active"',)),
t("J09","JS/DOM",js('document.getElementById("out").textContent=document.getElementById("field").value;','<input id="field" value="VALUE_OK"><div id="out">PENDING</div>'),('id="out">VALUE_OK</div>',)),
t("J10","JS/DOM",h('<div id="out">PENDING</div><script src="external.js"></script>'),('id="out">EXTERNAL_OK</div>',)),
t("J11","JS/async",js('Promise.resolve("RESOLVED").then(x=>{document.getElementById("out").textContent=x;});'),('id="out">RESOLVED</div>',)),
t("J12","JS/async",js('async function run(){return 42;}run().then(x=>{document.getElementById("out").textContent=x;});'),('id="out">42</div>',)),
t("J13","JS/DOM",js('document.getElementById("out").textContent=[1,2,3].reduce((a,b)=>a+b,0);'),('id="out">6</div>',)),
t("J14","JS/DOM",js('document.getElementById("out").textContent=String(Math.max(2,7));'),('id="out">7</div>',)),
t("J15","JS/DOM",js('document.getElementById("out").textContent=(({x:1})?.x)??7;'),('id="out">1</div>',)),
]

def rgb(ppm,x,y):
    with ppm.open("rb") as f:
        magic=f.readline().strip(); dim=f.readline().split(); depth=f.readline().strip(); data=f.read()
    w,h=map(int,dim)
    if magic!=b"P6" or depth!=b"255" or len(data)!=w*h*3: raise ValueError("invalid PPM")
    off=(int(y)*w+int(x))*3
    return list(data[off:off+3])

def run():
    if not BIN.is_file(): raise RuntimeError("release binary missing")
    for d in (PAGES,CAP): d.mkdir(parents=True,exist_ok=True)
    (PAGES/"tiny.ppm").write_bytes(b"P6\n2 2\n255\n"+bytes([200,10,30]*4))
    (PAGES/"external.js").write_text('document.getElementById("out").textContent="EXTERNAL_OK";\n',encoding="utf-8")
    digest=hashlib.sha256(BIN.read_bytes()).hexdigest()
    results=[]
    for c in cases:
        path=PAGES/(c["id"]+".html"); path.write_text(c["html"],encoding="utf-8")
        screenshot=CAP/(c["id"]+".ppm")
        command=[str(BIN),"--quiet","--dump-html","--size","120x80","--screenshot",str(screenshot),str(path)]
        row={"id":c["id"],"area":c["area"],"inputs_sha256":hashlib.sha256(path.read_bytes()).hexdigest(),
             "expected_dom":c["dom"],"expected_pixels":c["pixels"],"command":command}
        start=time.monotonic()
        try:
            p=subprocess.run(command,capture_output=True,text=True,errors="replace",timeout=15)
            checks=[{"kind":"DOM","expected":x,"found":x in p.stdout} for x in c["dom"]]
            if screenshot.exists():
                for x,y,color in c["pixels"]:
                    observed=rgb(screenshot,x,y)
                    checks.append({"kind":"PIXEL","at":[x,y],"expected":list(color),"actual":observed,"found":observed==list(color)})
            row.update(exit_code=p.returncode,stdout_sha256=hashlib.sha256(p.stdout.encode()).hexdigest(),
                stdout_excerpt=p.stdout[-900:],stderr_excerpt=p.stderr[-800:],
                image_sha256=hashlib.sha256(screenshot.read_bytes()).hexdigest() if screenshot.exists() else None,
                checks=checks,status="PASS" if p.returncode==0 and screenshot.exists() and checks and all(z["found"] for z in checks) else "FAIL")
        except Exception as e:
            row.update(status="FAIL",error=repr(e))
        row["elapsed_s"]=round(time.monotonic()-start,3)
        results.append(row)
        print(row["id"],row["area"],row["status"],flush=True)
    tally=collections.Counter(x["status"] for x in results)
    by_area={}
    for area in sorted({c["area"] for c in cases}):
        rows=[r for r in results if r["area"]==area]
        by_area[area]=dict(total=len(rows),passed=sum(r["status"]=="PASS" for r in rows),
                           failed=sum(r["status"]=="FAIL" for r in rows))
    report={"schema":"controlled-m2-r1-v1","release_tag":"v0.1.0",
            "release_sha":"a065c9d3f37ac573e937b4606a4ee3007016b0b2",
            "binary_sha256":digest,"platform":"Ubuntu 26.04 WSL2 headless CLI; no GUI evidence",
            "safety":"local trusted fixtures only; no live untrusted sites",
            "limitation":"not representative of real sites; no navigation GUI/HTTPS/IME tests",
            "total":len(results),"status":dict(tally),"areas":by_area,"cases":results}
    (OUT/"baseline.json").write_text(json.dumps(report,indent=2,ensure_ascii=False)+"\n",encoding="utf-8")
    (OUT/"summary.md").write_text("# M2-R1 v0.1.0 Controlled Baseline\n\n"
        +"Release binary SHA-256: "+digest+"\n\n"
        +"Cases: "+str(len(results))+"; PASS "+str(tally["PASS"])+"; FAIL "+str(tally["FAIL"])+"\n\n"
        +"| ID | Area | Result |\n|---|---|---|\n"
        +"".join("| "+r["id"]+" | "+r["area"]+" | "+r["status"]+" |\n" for r in results)
        +"\nThese are synthetic local cases, not real-website compatibility scores.\n",encoding="utf-8")
    print("SUMMARY",json.dumps({"status":dict(tally),"by_area":by_area}))
run()
