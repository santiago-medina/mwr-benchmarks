# MWR JS engine — estado verificado (14-11-2026)

Este es el **punto de entrada corto** para resultados verificados, distinto de la cronología de [CURRENT_STATUS.md](CURRENT_STATUS.md) y de las antiguas cifras de [IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md). **No es una declaración de conformidad ECMAScript al 100 %.**

## Última línea base completa validada

| Campo | Valor |
|---|---:|
| **Test262 aprobados** | **47.558 / 47.708** |
| Fallidos | 42 |
| No soportados | 108 |
| Regresiones respecto al baseline | 0 |
| Porcentaje del corpus fijado | **99,69 %** |

**Fuente:** Test262 `full` del [PR #34](https://github.com/santiago-medina/mwr-benchmarks/pull/34), [GitHub Actions run 43583692486](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43583692486), resultado reproducido sobre `integration/species-after-loop-bigint` en su último commit de CI, con el binario **Temporal-enabled** de `gc-lab/temporal/Cargo.toml`. La suite está fijada al commit Test262 `e4fbf57745c6aae3d1f12f181ecef78b1665d812`.

El código del PR #34 se fusionó en `codex/gc-host-boundary` mediante `c7d59a2fdecd43f8540f2af178bf70d564af5db6`, después de haber fusionado #32 como `3af91e7d4fc086d0c18b93c206adb5672b78c0a7`. **La ejecución completa posterior al merge** se enlaza desde [run 43584171885](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43584171885) (no asumir su conclusión sin consultar). El snapshot de #34 aporta la última cifra comprobada de la combinación antes de fusionar.

La mejora neta frente al checkpoint 46 de 47.458 aprobados es de **100 casos**. No sumar mejoras de ramas aisladas como si estuvieran integradas.

## Comprobaciones de ingeniería

- Contratos Rust en debug y release, formatos y perfiles estándar de Test262: **CI correcta**, [GC laboratory #34, run 43583711848](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43583711848).
- Integración de Servo mediante bridge y DOM/click real: **correcta en ese workflow**; **`servo-native` fue omitido**, y no debe declararse como prueba nativa exitosa.
- `Symbol.species`: cinco casos oficiales × Tree/VM × GC normal/estrés, **20/20 aprobados**, [run 43583688260](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43583688260).
- GC/VM smoke y Test262 dirigido no sustituyen un perfil `full` bajo GC stress. El `full` oficial se ejecuta en VM con GC convencional/por umbral; las matrices de estrés se ejecutan para casos seleccionados.

## Trabajo pendiente validable

- [PR #36](https://github.com/santiago-medina/mwr-benchmarks/pull/36): corrige `Object.assign.length` y el cálculo de `Function.prototype.bind` sobre una longitud **propia** del target. La versión original fue PR #35. Ambos necesitan 8/8 pruebas dirigidas y Test262 completo **en el árbol que incluye #32 + #34**, además de contratos y Servo; no fusionar hasta comprobarlo.
- [Triaje de 52 fallos en el SHA anterior](conformance/REMAINING_52_2026-11-14.md): informe inmutable [run 43577700483](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43577700483) anterior a #32 y #34. Esa fotografía contenía 52 fallos; la línea actual tiene 42. Debe actualizarse el triaje a partir del artifact `summary.json` más reciente, sin borrar los registros históricos.
- Los 108 `unsupported` requieren implementación y nuevas pruebas (parser y sintaxis, reflejo de hosts, módulos, `with` en generadores, UTF-16 no emparejado, etc.). **No** excluirlos del denominador para elevar el porcentaje.
- Dos casos `Promise.try` presentan una [discrepancia documentada entre expectativas de Test262 y el algoritmo normativo](conformance/SPEC_DIVERGENCES.md), corroborada con Node.js 24. No modificar semántica del motor para forzar su aprobación sin aclaración upstream.

## Criterio de merge

Todo cambio debe pasar: regresión específica (AST/VM y GC default/stress cuando aplique), tests Rust debug/release, perfiles habituales y Servo bridge, **Test262 completo con Temporal**, cero regresiones por ruta y comparación del informe individual frente a la última línea aprobada. El resultado debe asociarse siempre al SHA probado. `main` permanece fuera de esta auditoría hasta una decisión de release.
