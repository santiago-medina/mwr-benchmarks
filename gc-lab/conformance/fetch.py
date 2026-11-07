#!/usr/bin/env python3
"""Fetch the fixed Test262 snapshot, verifying archive bytes before extraction."""
import argparse,hashlib,tarfile,tempfile,urllib.request
from pathlib import Path
REVISION='e4fbf57745c6aae3d1f12f181ecef78b1665d812'
SHA256='01b5c93c8960a549375fd9d8085d06aadcc96cd29cf6bc137eff32ac98d592eb'
def main():
    p=argparse.ArgumentParser(__doc__);p.add_argument('--output',type=Path,default=Path(__file__).resolve().parents[1]/'.test262-source');a=p.parse_args()
    if (a.output/'.revision').exists() and (a.output/'.revision').read_text().strip()==REVISION:return
    a.output.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory() as temp:
        archive=Path(temp)/'test262.tar.gz';urllib.request.urlretrieve(f'https://codeload.github.com/tc39/test262/tar.gz/{REVISION}',archive)
        assert hashlib.file_digest(archive.open('rb'),'sha256').hexdigest()==SHA256
        with tarfile.open(archive) as tar:
            for member in tar:
                pieces=Path(member.name).parts
                if len(pieces)<2:continue
                member.name=str(Path(*pieces[1:]));tar.extract(member,a.output,filter='data')
    (a.output/'.revision').write_text(REVISION+'\n')
if __name__=='__main__':main()
