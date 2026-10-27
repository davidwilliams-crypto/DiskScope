"""Create a deterministic fresh benchmark folder; never deletes/overwrites data."""
import argparse
import json
from pathlib import Path

p=argparse.ArgumentParser()
p.add_argument('destination',type=Path)
p.add_argument('--folders',type=int,default=256)
p.add_argument('--files-per-folder',type=int,default=64)
a=p.parse_args()
if a.destination.exists():
    raise SystemExit('Choose a new destination; existing folders are never overwritten.')
if not (1<=a.folders<=4096 and 1<=a.files_per_folder<=4096):
    raise SystemExit('Fixture dimensions out of bounds.')
a.destination.mkdir(parents=True)
total=0
for d in range(a.folders):
    folder=a.destination/f'folder-{d:04d}'
    folder.mkdir()
    for f in range(a.files_per_folder):
        size=(d*127+f*311)%8193
        extension=['.rs','.txt','.jpg','.zip'][f%4]
        (folder/f'item-{f:04d}{extension}').write_bytes(bytes([f%251])*size)
        total+=size
manifest={'root':str(a.destination.resolve()),'files':a.folders*a.files_per_folder,'folders':a.folders,'logical_bytes':total,'notes':'Ordinary files only; no links, ADS, or sparse files. Separate scanner tests cover those cases.'}
out=a.destination.with_name(a.destination.name+'-manifest.json')
out.write_text(json.dumps(manifest,indent=2),encoding='utf-8')
print(json.dumps(manifest))
