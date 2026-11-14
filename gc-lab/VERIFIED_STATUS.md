# MWR JS engine — estado verificado (14-11-2026)

Este es el **punto de entrada corto** para resultados verificados, distinto de la cronología de [CURRENT_STATUS.md](CURRENT_STATUS.md) y de las antiguas cifras de [IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md). **No es una declaración de conformidad ECMAScript al 100 %.**

## Última línea base completa validada

| Campo | Valor |
|---|---:|
| **Test262 aprobados** | **47.560 / 47.708** |
| Fallidos | 40 |
| No soportados | 108 |
| Regresiones respecto al baseline | 0 |
| Porcentaje del corpus fijado | **99,69 %** |

**Fuente:** Test262 `full` del [PR #36](https://github.com/santiago-medina/mwr-benchmarks/pull/36), [GitHub Actions run 43584264246](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43584264246), resultado reproducido sobre `integration/function-length-after-species` en su último commit de CI, con el binario **Temporal-enabled** de `gc-lab/temporal/Cargo.toml`. La suite está fijada al commit Test262 `e4fbf57745c6aae3d1f12f181ecef78b1665d812`.

El código del PR #36 se fusionó en `codex/gc-host-boundary` mediante `dacddd4e8153864e7ed2e619c6de3a87c87e9fd8`, después de los merges #32 (`3af91e7d4fc086d0c18b93c206adb5672b78c0a7`) y #34 (`c7d59a2fdecd43f8540f2af178bf70d564af5db6`). La [ejecución full posterior al merge](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43584993578) se lanzó para ese árbol. No atribuirle una conclusión hasta revisar su informe. El snapshot de #36 acredita la última combinación validada antes de fusionar.

La mejora neta frente al checkpoint 46 de 47.458 aprobados es de **102 casos**. No sumar mejoras de ramas aisladas como si estuvieran integradas.

## Comprobaciones de ingeniería

- Contratos Rust en debug y release, formatos y perfiles estándar de Test262: **CI correcta**, [GC laboratory #36, run 43584304458](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43584304458).
- Integración de Servo mediante bridge y DOM/click real: **correcta en ese workflow**; **`servo-native` fue omitido**, y no debe declararse como prueba nativa exitosa.
- `Symbol.species`: cinco casos oficiales × Tree/VM × GC normal/estrés, **20/20 aprobados**, [run 43583688260](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43583688260).
- GC/VM smoke y Test262 dirigido no sustituyen un perfil `full` bajo GC stress. El `full` oficial se ejecuta en VM con GC convencional/por umbral; las matrices de estrés se ejecutan para casos seleccionados.

## Trabajo pendiente validable

- [PR #36](https://github.com/santiago-medina/mwr-benchmarks/pull/36) **fusionado**: `Object.assign.length` y `Function.prototype.bind` al leer solo la longitud propia del target; 8/8 pruebas dirigidas en cuatro modos [run 43584282392](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43584282392), full sin regresiones y contratos/Servo correctos.
- [PR #39](https://github.com/santiago-medina/mwr-benchmarks/pull/39) **en verificación**: `eval` directo en inicializadores por defecto de parámetros flecha. La matriz histórica fue 0/32 [run 43584592387](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43584592387); el parche consiguió 32/32 en la rama aislada [run 43584907425](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43584907425). Exigir el full del **árbol combinado** y CI Rust/Servo antes de merge.
- [Triaje de 52 fallos en el SHA anterior](conformance/REMAINING_52_2026-11-14.md): informe inmutable [run 43577700483](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43577700483) anterior a #32 y #34. Esa fotografía contenía 52 fallos; la línea actual tiene 40. Debe actualizarse el triaje a partir del artifact `summary.json` más reciente, sin borrar los registros históricos.
- Los 108 `unsupported` requieren implementación y nuevas pruebas (parser y sintaxis, reflejo de hosts, módulos, `with` en generadores, UTF-16 no emparejado, etc.). **No** excluirlos del denominador para elevar el porcentaje.
- Dos casos `Promise.try` presentan una [discrepancia documentada entre expectativas de Test262 y el algoritmo normativo](conformance/SPEC_DIVERGENCES.md), corroborada con Node.js 24. No modificar semántica del motor para forzar su aprobación sin aclaración upstream.

## Criterio de merge

Todo cambio debe pasar: regresión específica (AST/VM y GC default/stress cuando aplique), tests Rust debug/release, perfiles habituales y Servo bridge, **Test262 completo con Temporal**, cero regresiones por ruta y comparación del informe individual frente a la última línea aprobada. El resultado debe asociarse siempre al SHA probado. `main` permanece fuera de esta auditoría hasta una decisión de release.
