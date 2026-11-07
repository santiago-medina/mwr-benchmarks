# Frontera entre dos heaps y presupuesto de bytes

## Qué se ha implementado

`src/dual.rs` contiene dos instancias independientes de Heap, cada una con sus
slots, identidades, raíces, lista de huecos y tabla de localización. No se comparte
un arena de objetos entre DOM y JS. Ambas arenas pueden compactarse por separado
con IDs estables. La combinación de Model::Compact se rechaza: ese modelo invalida
tokens y no implementamos un protocolo adicional de relocalización de puentes.

Las relaciones entre objetos y los payloads del anfitrión se guardan en registros
administrados por Runtime. Ese registro no es una raíz. Cada Id incluye su dominio
y un handle con identidad del heap/asignación. Runtime es la única vía pública de
mutación: no expone las arenas para que un consumidor ejecute un GC independiente.

Esto sigue siendo una simulación: no hay DOM real, código JS, SpiderMonkey ni
Servo. Ambos heaps implementan nuestro protocolo; no hemos demostrado que un
recolector externo arbitrario pueda participar sin modificaciones.

## Protocolo de colección

1. Detener mutaciones de ambos dominios (el prototipo es monohilo).
2. Tomar las raíces externas de ambas arenas.
3. Recorrer hasta completar la alcanzabilidad por todas las aristas locales y
   cruzadas, sin liberar nada durante ese recorrido.
4. Entregar a cada arena la partición de sus IDs alcanzables como raíces
   temporales para ese ciclo. Ejecutar sus recolecciones y compactaciones.
5. Retirar registros/payloads muertos. Las raíces temporales no quedan instaladas.

Una referencia DOM -> callback NO es una raíz global. Si callback -> DOM cierra
un ciclo sin acceso desde una raíz externa, mueren ambos. Si cualquiera de los
extremos tiene una raíz externa, sobreviven ambos. Los IDs externos no cambian
al compactar y las entradas de tabla liberadas se reutilizan con identidad nueva.

La coordinación es stop-the-world y usa un trazado común sobre un registro de
puentes. NO son recolectores concurrentes autónomos, una colección incremental,
una integración con referencia contada de Servo ni un diseño de barreras de escritura.
Las aristas del experimento están en los registros del coordinador, no en una
estructura DOM externa opaca.

## Invariantes que debe revisar una persona

| Invariante | Dónde se mantiene |
| --- | --- |
| Un ID identifica un objeto de un solo dominio/runtime/asignación | Id, Heap::position y Runtime::value |
| Ninguna mutación crea una arista a un objeto inválido | Runtime::link |
| Los registros y las aristas por sí solos no actúan como raíces | Runtime::collect |
| No se barre un heap antes de conocer el conjunto vivo completo | Dos fases de Runtime::collect |
| Las operaciones asignadoras pueden recolectar, incluso al fallar | Runtime::allocate |
| Añadir una arista no recolecta y falla sin modificar el grafo | Runtime::link |
| Las raíces internas se liberan al retornar o desenrollar panic | Scope::drop |
| Los IDs sobreviven a la compactación y no reviven al reutilizar slots | Heap Indirect + números de identidad |

El caso importante de reentrada: la operación enraíza el callback activo, puede
llamar de nuevo a dispatch y devuelve un objeto JS que referencia al nodo DOM y
al resultado anidado. El llamador debe enraizar el resultado antes del siguiente
safepoint. Los scopes se verifican con retornos de error y con panic=unwind.

## Presupuesto de bytes: alcance exacto

Se contabiliza, por objeto vivo:

```
size_of<Record> + size_of<Id> + Heap::object_charge() + payload.len()
```

Cada arista añade `size_of<Id>`. Los payloads son vectores de bytes reales inicializados
a cero. El cálculo suma bytes lógicos de objetos, claves, registros, payloads y
aristas, usando tamaños de la plataforma. Las estructuras de apoyo se cuentan
conceptualmente una vez por objeto, aunque estén en asignaciones separadas.

NO es un límite duro del proceso: excluye capacidad sobrante de Vec/HashMap,
raíces, tablas, slots libres, buffers temporales del GC, stack, overhead del
asignador y código. Por eso no impide toda denegación de servicio. Las capacidades
retenidas pueden seguir altas tras liberar todos los objetos, aunque el cargo
lógico vuelva a cero. Tampoco mide el pico de memoria.

La cuota se aplica a nuevas asignaciones y aristas; bajarla no expulsa objetos
existentes. Una asignación que no cabe intenta colección coordinada y devuelve
ByteLimit si sigue sin caber. Un link que no cabe devuelve ByteLimit sin GC y sin
mutación. Esto evita invalidar o recoger el objeto recién devuelto antes de unirlo.

`try_reserve` permite propagar algunos fallos de reserva como AllocationFailed,
pero NO hemos hecho fallibles todas las asignaciones de std, el registro de raíces
o el backend. No hay garantía de recuperación ante OOM real del sistema.
La suma del cargo se recalcula para reducir invariantes de contabilidad; es O(n)
por operación y no es una propuesta de optimización para producción.

## Evidencia y reproducción

```sh
cargo test --offline
cargo test --release --offline
cargo run --bin dual_demo --release --offline > dual-results.csv
```

La suite completa tiene 30 tests: 20 anteriores y 10 nuevos. Los tests nuevos
incluyen las cuatro combinaciones de Stable/Indirect entre DOM y JS; raíz inicial
en cualquiera de los dominios; compactación física de ambas arenas; reentrada con
resultados objeto; retirada de listener con callback externo vivo; cuotas de
payload/aristas y overflow; fallos parciales y reintento; limpieza ante panic;
IDs de otro runtime/dominio; y 32 grafos por combinación comparados con un oráculo
independiente durante tres colecciones sucesivas (128 grafos, 384 colecciones).

`dual-results.csv` muestra el ciclo vivo, los resultados, su liberación, el fallo
de cuota con limpieza y el reintento, para las cuatro combinaciones. No contiene
mediciones de rendimiento y sus bytes NO son comparables directamente con la
capacidad retenida del benchmark de un solo heap.

La validación no equivale a prueba formal. No se han probado finalizadores,
WeakMap/ephemerons, concurrencia, serialización de callbacks ni ciclo de eventos.

## Decisión y siguiente frontera

Los IDs estables permiten separar representación física, raíces de ejecución y
trazado de referencias entre dominios. El coordinador encapsula la complejidad de
los puentes: no obliga a cada operación a actualizar direcciones tras compactar.

Para conectar con Servo hay que comprobar qué objetos puede enumerar su DOM y
cómo intercambia raíces con el GC de JS. No basta con envolver sus punteros en
este Id. La próxima prueba útil sería reemplazar el payload DOM simulado por un
adaptador de objetos anfitriones con callbacks explícitos de tracing, antes de
empezar a ejecutar JavaScript completo.
