#!/usr/bin/env python3
"""Reproducible, non-mutating triage of a pinned Test262 full summary.

This is a diagnostic: route-based groups are *hypotheses*, not proven root causes.
"""
import argparse
import collections
import json
from pathlib import Path


def family(path):
    if path.startswith("built-ins/Promise/try/"):
        return "Promise.try identity — verify against ECMA-262 and other engines"
    if path.startswith("built-ins/Promise/"):
        return "Promise combinators — iterator close / abrupt completion"
    if path.startswith("built-ins/AsyncFromSyncIteratorPrototype/"):
        return "AsyncFromSyncIterator — continuation / close / rejection"
    if path.startswith("built-ins/Date/prototype/toTemporalInstant/"):
        return "Date.toTemporalInstant — API/receiver/descriptor"
    if path.startswith("built-ins/Date/"):
        return "Date — conversion order"
    if path.startswith("language/eval-code/direct/") and "arguments" in path:
        return "Direct eval — lexical arguments in arrows"
    if path.startswith("language/eval-code/"):
        return "Eval — completion values"
    if path.endswith("/order-of-evaluation.js") and path.startswith("language/expressions/"):
        return "Binary operators — ToNumeric coercion order"
    if path.startswith("language/expressions/assignment/") or path.startswith("language/destructuring/"):
        return "Assignment/destructuring — references and evaluation order"
    if path.startswith("language/expressions/delete/"):
        return "Delete — property reference evaluation"
    if path.startswith("built-ins/"):
        return "Built-ins — " + path.split("/")[1]
    return "Language — " + "/".join(path.split("/")[:3])


def escape(text):
    return str(text).replace("|", "\\|").replace("\n", " ").replace("\r", " ")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("summary", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--previous", type=Path,
                        help="Optional previous full summary to report exact status transitions")
    args = parser.parse_args()
    summary = json.loads(args.summary.read_text())
    counts = summary["counts"]
    failures = summary["failures"]
    total = summary["total"]
    assert total == sum(counts.values()), "Count invariant failed"
    assert len(failures) == counts["failed"], "Failure entries and count disagree"
    assert len({c["path"] for c in failures}) == len(failures), "Duplicate paths"
    unsupported = summary.get("unsupported_reasons", {})
    assert sum(unsupported.values()) == counts["unsupported"], "Unsupported reasons mismatch"

    groups = collections.defaultdict(list)
    for item in failures:
        groups[family(item["path"])].append(item)
    groups = sorted(groups.items(), key=lambda kv: (-len(kv[1]), kv[0]))
    lines = [
        "# Test262 full — diagnóstico reproducible",
        "",
        f"Snapshot Test262: \`{summary['revision']}\`. Perfil: \`{summary['profile']}\`.",
        f"Motor SHA256: \`{summary.get('implementation_sha256', 'unknown')}\`.",
        "",
        "| Resultado | Casos |",
        "|---|---:|",
        f"| Passed | {counts['passed']:,} |",
        f"| Failed | {counts['failed']:,} |",
        f"| Unsupported | {counts['unsupported']:,} |",
        f"| Total | {total:,} |",
        "",
        "**Importante:** los grupos están inferidos por rutas. Deben validarse con",
        "reproducciones Tree/VM y GC stress antes de afirmar una causa raíz.",
        "Los casos unsupported no se contabilizan como aprobados.",
        "",
        "## Grupos de fallos reales",
        "",
        "| Posible familia | Fallos |",
        "|---|---:|",
    ]
    for label, entries in groups:
        lines.append(f"| {escape(label)} | {len(entries)} |")
    lines += ["", "## Inventario de los fallos", ""]
    for label, entries in groups:
        lines += [f"### {label} ({len(entries)})", ""]
        for e in entries:
            lines.append(f"- \`{e['path']}\` — {escape(e.get('reason', ''))[:200]}")
        lines.append("")

    lines += ["## Casos no soportados", "", "| Motivo | Casos |",
              "|---|---:|"]
    for label, n in sorted(unsupported.items(), key=lambda kv: (-kv[1], kv[0])):
        lines.append(f"| {escape(label)} | {n} |")
    lines.append("")

    if args.previous:
        previous = json.loads(args.previous.read_text())
        assert previous["revision"] == summary["revision"], "Test262 revision changed"
        before = {e["path"]: e["status"] for e in previous.get("failures", [])}
        now = {e["path"]: e["status"] for e in failures}
        introduced = sorted(set(now) - set(before))
        fixed = sorted(set(before) - set(now))
        lines += [
            "## Transiciones frente a la referencia suministrada",
            "",
            f"Casos que eran fallidos y ahora no: **{len(fixed)}**.",
            f"Casos fallidos nuevos frente a esa referencia: **{len(introduced)}**.",
            "",
            "Este cálculo solo compara los elementos \`failed\` de ambas listas.",
            "Una transición a \`unsupported\` requiere inspeccionar también \`cases.json\`.",
            "",
        ]
        lines += [f"- Nuevo fallo: \`{p}\`" for p in introduced]
        lines += [f"- Ya no falla: \`{p}\`" for p in fixed]
        lines.append("")

    lines += [
        "## Reglas de validación",
        "",
        "Antes de fusionar una corrección: reproducir casos dirigidos en Tree/VM ×",
        "GC normal/stress, pasar contratos Rust debug/release y ejecutar full con",
        "el binario Temporal habilitado. Confirmar rutas y estado de cada caso,",
        "no inferir corrección de un workflow verde que solo archiva fallos.",
        "La integración nativa de Servo no está cubierta por esta herramienta.",
        "",
    ]
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text("\n".join(lines) + "\n")
    print(f"triaged {len(failures)} failures across {len(groups)} families, "
          f"{counts['unsupported']} unsupported -> {args.output}")


if __name__ == "__main__":
    main()
