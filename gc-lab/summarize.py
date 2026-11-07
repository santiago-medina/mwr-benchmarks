"""Summarize the recorded CSV; Python standard library only."""
import csv
import statistics
from pathlib import Path

root = Path(__file__).resolve().parent
rows = list(csv.DictReader((root / 'results.csv').open()))
lines = ['# Resultados del laboratorio', '',
         'Medianas de nueve ejecuciones por combinación. MB decimales; capacidad estimada, no RSS.', '',
         '| Carga | Modelo | Capacidad MB | Tabla MB | GC µs | Lecturas µs |',
         '| --- | --- | ---: | ---: | ---: | ---: |']
for scenario in ['sparse', 'dense']:
    for model in ['Stable', 'Compact', 'Indirect']:
        selected = [r for r in rows if r['scenario'] == scenario and r['model'] == model]
        assert len(selected) == 9
        median = lambda k: statistics.median(int(r[k]) for r in selected)
        lines.append(f'| {scenario} | {model} | {median("retained_capacity_bytes") / 1e6:.3f} | '
                     f'{median("table_bytes") / 1e6:.3f} | {median("gc_us")} | {median("read_us")} |')
lines += ['', 'Lecturas: 200.000 en sparse, 2.000.000 en dense. Los tokens se resuelven antes del tramo medido.', '',
          '## Tamaño del código', '', '| Archivo | Líneas con comentarios y vacías |', '| --- | ---: |']
for p in sorted(root.rglob('*.rs')):
    if 'target' not in p.relative_to(root).parts:
        lines.append(f'| {p.relative_to(root)} | {len(p.read_text().splitlines())} |')
lines += ['', 'No se ha medido tiempo de revisión humana. Ver README.md para contratos, límites y metodología.', '']
(root / 'summary.md').write_text('\n'.join(lines))
print('\n'.join(lines))
