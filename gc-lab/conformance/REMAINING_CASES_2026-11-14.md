# Casos Test262 restantes — auditoría reproducible del 14-11-2026

**Fuente:** ejecución [43569991719](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43569991719), artefacto `mwr-test262-full-audit` (ID `14103851902`), archivo `summary.json` y lista individual `cases.json`. Commit probado: `2678c6f5dc2b9c316c70ffe6dcc503de7e1ddf2e`, posterior a la fusión del PR #12. Snapshot Test262: `e4fbf57745c6aae3d1f12f181ecef78b1665d812`. Binario de `gc-lab/temporal/Cargo.toml`, perfil `full`. Este es un **registro inmutable de ese SHA**, no una afirmación sobre ramas posteriores.

| Estado | Casos |
|---|---:|
| Passed | **47 498** |
| Failed | **102** |
| Unsupported | **108** |
| Total | **47 708** |

Aprobados: 99,56 % del corpus/perfil fijado, **no** 99,56 % de la especificación ECMAScript completa. Contra el checkpoint histórico `47 458/47 708`, la mejora neta es de 40 aprobados. Los grupos siguientes proceden de los **102 fallos**, sin mezclar los 108 `unsupported`.

## Fallos que conviene abordar por familias

| Familia de rutas | Fallos | Ejemplos / siguiente verificación |
|---|---:|---|
| `language/expressions/*` | 19 | Once `order-of-evaluation.js` en operadores bitwise/aritméticos; 5 de asignación, 3 de delete. Distinguir compilación VM de semántica compartida mediante ejecución Tree/VM, y observar evaluación de LHS, ToPropertyKey y GetValue |
| `built-ins/Promise/*` | 17 | `all`, `allSettled`, `race`: cierre/errores del iterador y `then`. Dos casos `Promise.try` están catalogados como posible divergencia Test262/especificación: **no modificar semántica solo para aprobarlos**, ver `SPEC_DIVERGENCES.md` |
| `language/eval-code/*` | 10 | Ocho pruebas de `arguments` en funciones flecha con eval directo; dos de `cptn-nrml-empty-for`. Revisar captura de bindings y Completion Records sin confundirlos con los catorce casos ya corregidos |
| `built-ins/Date/*` | 9 | Ocho `Date.prototype.toTemporalInstant` (métodos/descriptores/receivers), y un `Date.UTC` de orden de coerción |
| `built-ins/AsyncFromSyncIteratorPrototype/*` | 8 | Rechazos de Promise, envoltorios envenenados y cierre del iterador: diagnosticar orden y resultado de AsyncFromSyncIteratorContinuation |
| `language/statements/*` | 8 | Async generators, `for`, `for-await-of`, `try`, `with` y clases con Symbol; probablemente no son una sola causa |
| `built-ins/Object/*` | 7 | Tres `Object.prototype.toString` y tres métodos `values/entries/getOwnPropertyDescriptors` con global alterado, más `Object.assign` length. El PR #13 aborda solo `toString`; verificar su prueba completa antes de fusionar |
| `built-ins/BigInt/*` | 3 | Constructor, atributos de `toString` y llamada al prototipo |
| `built-ins/Function/*` | 3 | `bind` length y `toString` de funciones |
| `language/types/*` | 3 | Dos casos de number y uno de undefined |
| Otros | 15 | Map (2), RegExp (2), Set (2), Symbol (2), TypedArray (2), AsyncFunction (1), GeneratorPrototype (1), TypedArrayConstructors (1), destructuring (1), global-code (1) |

**Prioridad sugerida:** primero los 11 tests de orden de evaluación, porque pueden compartir infraestructura de referencias/operandos; en paralelo investigar los 8 AsyncFromSyncIterator y los 15 casos de `Promise.all/allSettled/race` separados de `Promise.try`. Son hipótesis a verificar, no fallos de causa raíz confirmada. En cada cambio registrar paths concretos, Tree/VM, GC normal/stress y Test262 completo con comprobación de casos perdidos.

## Los 108 no soportados

Razones principales extraídas de `summary.json`, según la clasificación del runner:

| Razón | Casos |
|---|---:|
| `syntax: unsupported character` (+ 3 en strict) | 23 |
| `syntax (strict): unsupported property key` | 12 |
| `ordinary reflection on host objects` | 11 |
| `unsupported module syntax or type` | 11 |
| `with in generator or async function` | 10 |
| `unpaired UTF-16 string storage` | 9 |
| `syntax: binding declaration requires initializer` | 8 |
| `ShadowRealm.prototype.importValue` | 6 |
| Otras razones sintácticas y límites de anidamiento | 18 |

La clasificación `unsupported` no equivale a resultado aprobado ni a bug confirmado: necesita triaje separado por gramática, representación, host y módulos.

## Límites de la evidencia y próximos gates

1. Los `new_failures` de `summary.json` no son una lista de regresiones respecto al checkpoint histórico: en ese runner los casos no incluidos en `known-failures-full.json` pueden aparecer como nuevos incluso si ya fallaban antes. Comparar estados por ruta y commit, no solo este campo.
2. `regressions: []` necesita interpretarse junto con las comprobaciones del workflow y el baseline realmente presente; la ejecución citada terminó correctamente y el recuento mejoró, pero esa señal por sí sola no prueba equivalencia total.
3. Tras merge al branch de desarrollo `codex/gc-host-boundary` (PR #15), la CI y el `full` están arrancados para ese SHA. No atribuir sus resultados a este documento hasta obtenerlos.
4. `servo-native` sigue fuera de la ejecución ordinaria de GitHub Actions. No declarar una prueba nativa completa sin una ejecución explícita.
