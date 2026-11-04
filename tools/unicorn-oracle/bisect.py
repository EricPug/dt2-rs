import subprocess,re,sys
base="RGPIO_READY_MODEL GPIO_LOOPBACK ESDHC_EMMC INTC1_BOOTSTRAP SSI_AUDIO_IRQ=100000 DTIM3_BOOTSTRAP DTIM3_TICKS_PER_INSTRUCTION=16 SNAPSHOT_LOAD=out/panel-1p5b.snap".split()
def dt2(n):
    o=subprocess.run(["tools/run-section3.sh","target/release/dt2-cli",str(n+1)]+base,capture_output=True,text=True).stdout
    r={}
    for m in re.finditer(r"\b([DA][0-7]|PC|SR)=0x([0-9a-f]+)",o): r[m.group(1)]=int(m.group(2),16)
    return r
def uc(n):
    o=subprocess.run(["/tmp/ucw/h",str(n)],capture_output=True,text=True).stdout
    r={}
    for m in re.finditer(r"\b([DA][0-7]|pc|sr)=([0-9a-f]+)",o): r[m.group(1).upper()]=int(m.group(2),16)
    return r
lo,hi=int(sys.argv[1]),int(sys.argv[2])
while hi-lo>1:
    mid=(lo+hi)//2
    if dt2(mid)==uc(mid): lo=mid
    else: hi=mid
print("first diff at N=",hi)
a,b=dt2(hi-1),uc(hi-1);print("last good",hi-1,a==b)
a,b=dt2(hi),uc(hi)
print({k:(hex(a[k]),hex(b.get(k,-1))) for k in a if a[k]!=b.get(k)})
