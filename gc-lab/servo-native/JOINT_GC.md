# Marcado entre SpiderMonkey y nuestra VM

Validación local: 1.500 ciclos, 3.000 botones nativos finalizados, 1.500 callbacks y
89 errores recuperados y dos navegaciones reales. El realm permanece vivo: vuelve a 4 objetos propios y un
wrapper nativo legítimo después de CADA ciclo. Nunca se llama release_globals.
El resultado medido está en `../native-gc-proof.json`.

## Aristas y raíces

El registro ya no guarda Trusted<Node> permanentes. Un handle contiene Dom<Node>
y un Weak de un token alojado en NodeRareData. El finalizador nativo destruye el
token de liveness; desde ese momento se rechaza el handle antes de acceder al puntero. El
token no posee el nodo. Su indicador atómico de destrucción puede observarse
desde otro hilo sin transferir referencias al DOM. Los IDs no se reutilizan. El objeto Rust no se mueve; el
trazador de Servo actualiza el reflector JS cuando SpiderMonkey lo mueve.

El root tracer de Servo recorre las aristas nativas alcanzables desde raíces
REALES de nuestra VM. Al marcar un nodo con listener extranjero, SpiderMonkey
llama al hook del listener: este recorre su closure y entorno en nuestra VM y
traza los reflectores nativos alcanzados. La cola de marcado nativa continúa el
recorrido hasta agotar las aristas. No se usa is_connected como oráculo del GC.

Durante una llamada activa, el adaptador mantiene pins nativos temporales y la
VM conserva frames y retenciones provisionales de listeners. Al suspender la VM
se retiran todos esos pins nativos. Las retenciones provisionales de listeners
NO son raíces iniciales del marcado conjunto: hacerlo conservaría los ciclos.

Al habilitar MWR_NATIVE_JS se fuerzan a false las preferencias de GC incremental
y por zonas: esta frontera todavía no tiene las barreras necesarias para ellas.
El safepoint coordinado exige que todas las VMs del hilo estén suspendidas y sin
pins activos. Ejecuta JS_GC (colección completa no incremental) y después barre
nuestro heap usando sus raíces reales y los wrappers alcanzados por el marcado
nativo. Solo entonces se purgan metadatos y handles débiles. Ningún callback de
trazado ejecuta JS, invoca el DOM o asigna objetos de nuestra VM. Los borrows de
RefCell se terminan antes de llamar al trazador nativo.

## Prueba sobre objetos reales

El programa mantiene un botón desconectado en una variable global. Debe sobrevivir
TODAS las colecciones. En cada ciclo, una función crea una closure local sobre
un botón conectado y otro botón desconectado; ninguno queda en variables globales.
Tras volver al event loop, se recoge: DOM → listener → VM → segundo nodo debe
seguir vivo, y el callback modifica ambos botones reales. Se retira el primero,
se terminan los frames nativos y se vuelve a recoger en una tarea posterior.

Se observa la expiración de los dos tokens causada por finalizadores de Servo,
no solo la retirada de entradas del registro. La closure y su entorno desaparecen
del arena y de los metadatos. Los IDs antiguos se rechazan. Los DomIDs de Runtime representan wrappers: no se confunde su arena con el DOM
nativo de Servo. Cada 17 ciclos la
instalación lanza un error después de registrar el listener: este debe continuar
funcionando y recogerse después.

Antes de comprobar ausencia de raíces nativas, el harness sincroniza layout con
una consulta normal. Las colas de restyle pueden poseer nodos desconectados hasta
consumir ese trabajo. Se permiten hasta cinco colecciones/tareas para drenar
basura flotante; no se borran raíces de Servo para forzar el resultado.

Tras los ciclos se navega dos veces con la API nativa de Servo y reemplazo de
historial. El siguiente documento observa, mediante flags atómicos, la destrucción
de la sesión anterior y la finalización de su documento y de su nodo global.
Estos observadores no conservan Rc, Dom, Window, contexto JS ni VM anteriores.

## Reproducir y límites

```sh
python3 gc-lab/servo-native/run_gc.py --servo /ruta/al/servoshell-compilado
```

El harness está en mwr_gc_test.rs, separado del protocolo de integración mwr.rs.
El runner solo lanza el proceso y recoge evidencia; no usa WebDriver ni eval.
Los clicks son sintéticos mediante HTMLElement::Click en Rust.

Es un protocolo experimental de safepoints completos sobre un único hilo. Los
GC ordinarios de SpiderMonkey trazan las aristas, pero el barrido de nuestra VM
se coordina explícitamente o mediante una tarea posterior a nuestras invocaciones.
Un GC ordinario aislado puede dejar retenciones conservadoras de metadatos hasta
el siguiente safepoint coordinado. Los contadores validan objetos vivos del
experimento; no son una medición de RSS ni una auditoría de todas las fugas de Servo.
No hay integración incremental/concurrente ni barreras de escritura completas.
Quedan fuera adopción de nodos entre realms, workers y objetos Event dentro de
nuestra VM. La VM mantiene su subconjunto explícito de sintaxis.
