"""Download the exact Servo binary used by the integration experiment."""
import argparse
import hashlib
from pathlib import Path
import subprocess
import tarfile

VERSION = 'v0.7.0'
SHA256 = '728eba1be1cc1851e05dfaa90e18f98ab8644dd2355bedb0b8a5e89795ebe333'
URL = f'https://github.com/servo/servo/releases/download/{VERSION}/servo-x86_64-linux-gnu.tar.gz'

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--directory', default='gc-lab/.servo-test')
    args = parser.parse_args()
    directory = Path(args.directory).resolve()
    directory.mkdir(parents=True, exist_ok=True)
    archive = directory / 'servo.tar.gz'
    if not archive.exists() or hashlib.sha256(archive.read_bytes()).hexdigest() != SHA256:
        subprocess.run(['curl', '-fsSL', '--retry', '3', '--max-time', '120', URL, '-o', str(archive)], check=True)
    if hashlib.sha256(archive.read_bytes()).hexdigest() != SHA256:
        raise RuntimeError('Servo archive SHA-256 mismatch')
    with tarfile.open(archive) as package:
        package.extractall(directory, filter='data')
    print(directory / 'servo/servoshell')

if __name__ == '__main__':
    main()
