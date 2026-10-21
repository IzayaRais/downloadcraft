#!/usr/bin/env python3
"""Verify universal Mach-O architecture/minimum OS and ad-hoc code-page hashes."""
import argparse,hashlib,json,struct
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('binary',type=Path);a=p.parse_args();data=a.binary.read_bytes()
assert data[:4]==b'\xca\xfe\xba\xbe','Expected universal Mach-O'
count=struct.unpack_from('>I',data,4)[0];results=[]
for i in range(count):
 cpu,subtype,offset,size,align=struct.unpack_from('>IIIII',data,8+20*i)
 thin=data[offset:offset+size];assert thin[:4]==b'\xcf\xfa\xed\xfe','Expected 64-bit little-endian Mach-O'
 commands=struct.unpack_from('<I',thin,16)[0];pos=32;minimum=None;signature=None
 for _ in range(commands):
  cmd,length=struct.unpack_from('<II',thin,pos);assert length>=8
  if cmd==0x32:
   value=struct.unpack_from('<I',thin,pos+12)[0];minimum=(value>>16,(value>>8)&255,value&255)
  if cmd==0x1d:signature=struct.unpack_from('<II',thin,pos+8)
  pos+=length
 assert minimum and minimum<=(12,0,0),f'Unexpected minimum macOS {minimum}'
 assert signature,'Missing ad-hoc executable signature'
 signature_start,signature_size=signature;blob=thin[signature_start:signature_start+signature_size]
 magic,total,entries=struct.unpack_from('>III',blob);assert magic==0xfade0cc0
 directories=[]
 for entry in range(entries):
  kind,start=struct.unpack_from('>II',blob,12+entry*8)
  if struct.unpack_from('>I',blob,start)[0]!=0xfade0c02:continue
  cd=blob[start:];flags,hash_offset,ident,special,slots,limit=struct.unpack_from('>IIIIII',cd,12)
  hash_size,hash_type,platform,page=struct.unpack_from('BBBB',cd,36)
  assert flags&2,'Expected ad-hoc signature';assert hash_type==2 and hash_size==32,'Expected SHA-256 signature'
  for slot in range(slots):
   actual=hashlib.sha256(thin[slot*(1<<page):min((slot+1)*(1<<page),limit)]).digest()
   assert actual==cd[hash_offset+slot*hash_size:hash_offset+(slot+1)*hash_size],f'Invalid code signature page {slot}'
  directories.append(slots)
 assert directories,'No CodeDirectory found'
 results.append({'architecture':{0x1000007:'x86_64',0x100000c:'arm64'}.get(cpu,hex(cpu)),'minimum_macos':'.'.join(map(str,minimum)),'verified_signature_pages':sum(directories)})
assert {r['architecture'] for r in results}=={'x86_64','arm64'}
print(json.dumps(results,indent=2))
