# usage: DECODE_PROBE=<decode_probe binary> python3 opcode-diff.py <opcodes.txt>; diffs the emulator decode of executed opcode words against Capstone (needs the capstone package).
import capstone,subprocess,collections,sys,json
import os
R=os.path.join(os.path.dirname(os.path.abspath(__file__)),"..","out")+"/"
img=open(R+'section3-pre-entry-image.bin','rb').read()
sram=open(R+'section3-panel-discovery-pre-entry-sram.bin','rb').read()
def mem(pc,n=12):
    if 0x40000400<=pc<0x40000400+len(img): return img[pc-0x40000400:pc-0x40000400+n]
    if 0x80000000<=pc<0x80010000: return sram[pc-0x80000000:pc-0x80000000+n]
    return None
ops=[l.split() for l in open(sys.argv[1])]
rows=[];skipped=[]
for op,pc in ops:
    pc=int(pc,16);b=mem(pc)
    if b is None or int(op,16)!=int.from_bytes(b[:2],'big'): skipped.append((op,hex(pc)));continue
    rows.append((op,pc,b))
print('distinct executed opcode words',len(ops),'checked',len(rows),'skipped (code not in static image)',len(skipped),skipped[:10])
inp=''.join(f"{op} {b.hex()}\n" for op,pc,b in rows)
out=subprocess.run([os.environ.get('DECODE_PROBE','/tmp/dt2-ops/target/release/examples/decode_probe')],input=inp,capture_output=True,text=True).stdout.split('\n')
emu={l.split()[0]:l.split()[1:] for l in out if l}
md=capstone.Cs(capstone.CS_ARCH_M68K,capstone.CS_MODE_M68K_040|capstone.CS_MODE_BIG_ENDIAN)
cats=collections.defaultdict(list)
FLOW=('bra','bsr','jmp','jsr','rts','rte','trap','trapf','stop','halt','dbra','bt','bf','b')
for op,pc,b in rows:
    cs=list(md.disasm(b,pc,1)); e=emu[op]
    cinfo=(cs[0].size,f"{cs[0].mnemonic} {cs[0].op_str}") if cs else None
    flow=cinfo and (cinfo[1].split()[0].split('.')[0] in FLOW or cinfo[1].startswith('b'))
    if e[0]=='ILL': cats['emulator_rejects_executed'].append((op,hex(pc),cinfo,e))
    elif cinfo is None: cats['capstone_invalid_emulator_accepts'].append((op,hex(pc),b[:6].hex(),e))
    elif e[0]=='OK' and not flow and int(e[1])!=cinfo[0]: cats['length_mismatch'].append((op,hex(pc),cinfo,e))
for k,v in cats.items():
    print('==',k,len(v))
    for x in v: print('  ',*x)
