# Context Folding en Zest

Viabilidad e implementación de `fold()` para agentes que corren sobre el harness propio, no sobre puente CLI.

> **En espera.** Este plan queda parado hasta que termine la migración a Rig (`rig-migration.md`). No implementar antes: la fase 1 de esa migración invalida la premisa de la sección de alcance, porque la ruta Codex pasa a mandar items de reasoning (`Include::ReasoningEncryptedContent`) y aquí está escrito que no manda ninguno. Revisar esa sección antes de retomar.

## Correcciones sobre la versión anterior de este plan

Cuatro puntos que cambian el diseño. Tres eran errores míos; el cuarto es una verificación que confirma tu decisión.

**La serialización XML incremental sí es compatible con la Messages API.** Leí "serializar a XML" como "aplanar todo a un blob de texto", y no es eso. En el mecanismo de `fold.md`, el harness nunca envuelve la salida del modelo: la etiqueta `<model>` se emite al final del mensaje externo anterior y `</model>` al principio del mensaje externo siguiente. El contenido del assistant queda intacto entre las dos. Con eso caen tres de mis cuatro objeciones: las firmas de thinking no se tocan, el prefijo cacheado sigue siendo estable porque las etiquetas se escriben una sola vez y nunca se reescriben, y los bloques de imagen conviven sin problema porque siguen siendo bloques nativos.

**`model-metadata` se mantiene.** Mi razón para eliminarlo era falsa: no modifica la salida del agente, se inyecta en el bloque de usuario siguiente. Con él, los turnos del modelo sí reciben ID direccionable, `from` y `retain[]` pueden apuntar a ellos, y el archivo del fold consolida a `<model id= tokens=>` como describe la especificación.

**Thinking se descarta, y hay un mecanismo soportado para hacerlo.** Verificado contra la documentación actual, con una condición sobre el turno en vuelo. Está en la sección siguiente.

**Los tres providers nativos no son un solo régimen.** Traté todo el análisis de thinking como si aplicase a los tres, y solo aplica a `AnthropicProvider`. Los otros dos ni siquiera envían el razonamiento de vuelta hoy. Detalle en la sección de alcance.

La única objeción que sobrevive en los tres es el emparejamiento `tool_use` / `tool_result`, y no es sobre el marcado XML sino sobre la mecánica del reemplazo de rango.

## Alcance: dos regímenes, no uno

Solo providers con `Provider::owns_agent_loop() == false`, que son los que reciben la historia de wire como estructura editable. Pero entre ellos hay una división que importa para `fold`:

| Provider | Wire | Razonamiento en la petición |
| --- | --- | --- |
| `AnthropicProvider` | Messages API | Bloques `thinking` con `signature`, devueltos verbatim |
| `CodexOAuthProvider` | Codex Responses (`store: false`) | Ninguno |
| `OpenAiCompatibleProvider` | `/chat/completions` | Ninguno |

Para ChatGPT vía Codex sobre el harness nativo, `codex_oauth.rs::responses_input` traduce `Vec<Message>` a items de Responses y solo reconoce `text`, `tool_use` y `tool_result`. Cualquier bloque `thinking` cae en el brazo `_ => {}` y se descarta en silencio. El cuerpo lleva `store: false`, no manda `previous_response_id`, no pide `include: ["reasoning.encrypted_content"]`, y `ProviderSessionRef` no tiene variante para este provider. Es decir, la petición es completamente sin estado en lo que respecta al razonamiento.

`OpenAiCompatibleProvider::convert_messages` hace lo mismo, con el mismo `_ => {}`, contra `/chat/completions`.

Consecuencia directa: **todo lo que sigue sobre thinking, chequeo de prefijo y `drop_block` aplica solo a `AnthropicProvider`.** En las otras dos rutas un fold es un reemplazo de array de mensajes y nada más. Es la ruta más simple de las tres, no la más difícil.

Nota aparte, no es tarea de fold: que `responses_input` descarte el razonamiento significa que la ruta Codex ya pierde continuidad de razonamiento entre rondas de tools hoy. Fold no lo empeora. Si alguna vez se quiere recuperar, el camino es `include: ["reasoning.encrypted_content"]` con los items de reasoning reinyectados, y entonces sí habría que revisar esta sección.

Fuera de alcance en los tres casos: `ClaudeCodeProvider`, `CodexAppServerProvider`, `CursorAcpProvider`. El registro se condiciona con el `!provider_owns_agent_loop` que ya existe en `RuntimeBuilder`.

## Thinking en la ruta Anthropic

Todo lo de esta sección es específico de `AnthropicProvider`.

Fuentes: [Thinking](https://platform.claude.com/docs/en/build-with-claude/thinking), [Thinking in tool and multi-turn workflows](https://platform.claude.com/docs/en/build-with-claude/thinking-tool-workflows), [Preserved thinking](https://platform.claude.com/docs/en/build-with-claude/preserved-thinking), [Context editing](https://platform.claude.com/docs/en/build-with-claude/context-editing).

**Descartar thinking de turnos anteriores está explícitamente permitido.** La página de Thinking lo dice literalmente: "Allowed: outside tool use, omit prior turns' thinking", y "It is only strictly necessary to send back thinking blocks when using tools with thinking". Anthropic además publica la estrategia de context editing `clear_thinking_20251015`, con `keep: {type: "thinking_turns", value: N}`, que hace exactamente eso del lado servidor.

**La excepción es el turno en vuelo.** Dentro de un bucle de tools, el mensaje del assistant que lleva el `tool_use` debe volver con sus bloques de thinking intactos: "when you return a tool result, the thinking blocks from the assistant message must come back with it", y filtrarlos (incluidos los `redacted_thinking`) devuelve 400.

Esto encaja gratis con el diseño de `fold`: el rango replegado siempre termina **antes** del mensaje del assistant que emitió la llamada a `fold`, porque ese mensaje tiene que seguir presente para que su `tool_result` sea válido. El turno en vuelo conserva su thinking sin que haya que hacer nada.

**El obstáculo real está en otro sitio: el chequeo de prefijo.** A partir de Claude Fable 5.1, la API valida la firma de cada bloque de thinking contra todo lo enviado antes de él. La tabla "What counts as an edit" es tajante:

| Cambio entre peticiones | Bloques de thinking posteriores |
| --- | --- |
| Añadir mensajes al final | Válido |
| Quitar bloques `thinking` del **principio** del historial | Válido |
| Compactación o context editing **del lado servidor** | Válido |
| Añadir, mover o quitar marcadores `cache_control` | Válido |
| Editar, reordenar o borrar cualquier mensaje anterior | **Inválido** |
| Quitar un bloque `thinking` **del medio** conservando los posteriores | **Inválido para todos los posteriores** |
| Cambiar el `system` de nivel superior, o los `tools` | **Inválido** |

Un fold es precisamente lo que la documentación llama **keep-tail compaction**, y la nombra como el patrón que falla: "As usually written it breaks the rule: the kept assistant turns still carry thinking blocks that were produced when the original turns, not the summary, came before them. Those blocks fail."

Y da el arreglo exacto: "keep the turns exactly as they are and send `prefix_mismatch_behavior: "drop_block"`. The API drops the stale thinking blocks, the model reads the kept turns' `text` and `tool_use` blocks, and the request succeeds."

Es decir: la decisión de descartar thinking es la correcta, y además es la que la propia API espera para este patrón.

**Alcance temporal del chequeo.** El chequeo de modelo aplica a todas las cuentas. El de prefijo se aplica por defecto solo a cuentas creadas a partir del 31 de agosto de 2026 a las 00:00 UTC; en cuentas anteriores solo se aplica si la petición envía `thinking.block_binding.prefix_mismatch_behavior`. La documentación advierte: "Later models will enforce the prefix check for all accounts". Con `claude-opus-5` y una cuenta antigua, un fold funciona hoy sin tocar nada. En cuanto el usuario seleccione Fable 5.1, o cuando el chequeo se generalice, deja de funcionar.

**Consecuencia inmediata para Zest, independiente de fold.** `prune::prune_tool_results` reescribe cuerpos de `tool_result` en mensajes de usuario anteriores y conserva los turnos de assistant posteriores con su thinking. Eso es exactamente la fila "editar cualquier mensaje anterior" y ya es un fallo latente hoy bajo el chequeo de prefijo, en la rama `Pruned` de `compact_context` (que no reemplaza el historial, solo lo recorta). La rama `Summarized` está a salvo porque reemplaza todo por dos mensajes nuevos, que es justo la "simple compaction" que la documentación recomienda.

## Serialización incremental en Zest

El marcado se emite **solo desde los mensajes de rol `user`**, que son los que el harness tiene permitido construir. El assistant nunca se toca.

**Todo el marcado de un mensaje va en el payload de texto de su primer bloque**, sea ese bloque un `text` o el string `content` de un `tool_result`. No se añaden bloques `text` sueltos alrededor de un `tool_result`.

Un mensaje de usuario que transporta un resultado de tool queda así:

```json
[
  {
    "type": "tool_result",
    "tool_use_id": "toolu_…",
    "content": "</model>\n<model-metadata id=\"06T195410.184\" tokens=\"18\" />\n<tool-result id=\"06T195411.029\" tokens=\"620\">\nexport function AuthProvider() {\n…\n}\n</tool-result>\n<model>"
  }
]
```

Un turno de usuario normal es el mismo contenido en un único bloque `text`, con `<user_query id= tokens=>` en lugar de `<tool-result>`. Con varios resultados en paralelo, `</model>` y `<model-metadata/>` van en el primero y `<model>` en el último; los intermedios llevan solo su propio `<tool-result>`.

Renderizado, el modelo lee exactamente la secuencia de `fold.md`. Estructuralmente sigue siendo un `tool_result` nativo con su `tool_use_id`, así que el emparejamiento que la API valida queda intacto.

Esta forma la eligió el régimen de providers, no la estética:

**`responses_input` descarta el texto de un mensaje que lleva `tool_result`.** En `codex_oauth.rs` la rama `if !tool_results.is_empty()` emite un `function_call_output` por resultado y hace `continue` antes de usar `text`. `convert_messages` en `openai_compatible.rs` tiene la misma estructura. Un `<model-metadata/>` puesto como bloque `text` hermano se perdería en silencio en las dos rutas, y los turnos del modelo se quedarían sin ID justo ahí. Dentro del `content` del `tool_result` viaja en `output` y sobrevive a las tres.

**Y de paso desaparece la única incógnita empírica del plan anterior.** Ya no hay que comprobar si bloques `text` conviven con `tool_result` en un mismo mensaje de usuario, porque no se emiten. Un mensaje de resultados vuelve a ser solo bloques `tool_result`, que es también la forma que exige programmatic tool calling.

**El marcado sobrevive a `prune` y a `spill`.** `prune.rs` documenta y depende de que ese cuerpo sea un string plano, y `SpillPolicy` ya le añade su aviso al final. Con `PRUNE_HEAD_CHARS = 4_096` y `PRUNE_TAIL_CHARS = 1_024`, la apertura y el cierre sobreviven al recorte.

**No sobrevive a `redact_sensitive_staged`, y hay que arreglarlo.** Esa función reemplaza el `content` entero por `REDACTED_SENSITIVE_RESULT`, lo que borraría el ID del resultado y el `model-metadata` del turno anterior. Como `messages_for_persist` es lo que se guarda en `Thread::agent_messages` y lo que se restaura al reabrir, un hilo con un resultado sensible volvería con un agujero en el direccionamiento. La redacción debe conservar el sobre y sustituir solo el cuerpo entre `<tool-result …>` y `</tool-result>`.

**El `<model>` de apertura es redundante en Zest y aun así lo emito.** El rol `assistant` ya marca esa frontera. Cuesta unos dos tokens por turno y mantiene una gramática consistente entre el contexto vivo y el archivo del fold, que es lo que el agente vuelve a leer después. Si el coste importa, es lo primero que se puede quitar sin perder direccionabilidad.

## IDs

`fold.md` propone `DDTHHMMSS.mmm`. Dos defectos reales: colisiona entre meses y no es único bajo concurrencia, porque un batch de tools paralelo puede resolver en el mismo milisegundo. `thread.rs:40` ya resuelve esto con nanos más contador atómico.

`fold::entry_id()` produce `DDTHHMMSS.mmm` y desempata con sufijo `-1`, `-2` reutilizando el mismo `ID_SEQ`. Se conserva la legibilidad temporal que motiva el formato y se elimina la colisión.

El ID vive en el texto que ya se emite, no en una tabla lateral. Una tabla paralela obligaría a mantener sincronía a través de `prune_tool_results`, `redact_sensitive_staged`, `compact_context`, `set_agent_messages` y la restauración de checkpoints, que es el tipo de doble fuente de verdad contra el que argumenta el preámbulo de `provider/driver.rs`. Con el ID en el cuerpo, `Thread::agent_messages` ya lo persiste y la recarga del hilo no necesita nada.

`model-metadata` sale gratis por construcción: cuando el harness va a construir el siguiente mensaje de usuario, el turno del modelo ya terminó, así que su ID y su estimación de tokens son conocidos. Es exactamente el orden temporal que describe `fold.md`.

El nombre de archivo lleva fecha completa, `20260906T195527-<slug>.xml`, y pasa por la misma validación de segmento único que `SpillStore::write`.

## `fold` no puede ser un `Tool` normal

`ToolRegistry::execute_prepared` y `Agent::execute_tool_calls` toman `&self`, y un `Tool` devuelve `ToolOutcome` sin acceso a la historia en construcción. `fold` tiene que mutar `staged`, que solo existe dentro de `send_user_cancellable`.

Precedente exacto: `ASK_USER_TOOL` está interceptado por nombre en `execute_tool_calls` (`agent.rs:820`) con slot exclusivo, resuelto antes del batch concurrente.

`FOLD_TOOL` se declara con un `Tool` de fachada, para que entre en `tools.definitions()` y por tanto en el prefijo cacheado, pero se intercepta y se ejecuta en el bucle del turno. Slot exclusivo: un solo `fold` por ronda, el segundo devuelve error al modelo.

## Mecánica del reemplazo

Opera sobre índices de `staged`, nunca dentro de un `Message`.

1. Resolver `from` al índice del `Message` que lleva ese ID.
2. Ajustar hacia atrás hasta frontera de turno: si `staged[k]` es un mensaje de usuario compuesto solo por bloques `tool_result`, retroceder hasta el primer mensaje de usuario que no lo sea. Sin esto se deja un `tool_use` sin resultado y la petición siguiente es inválida.
3. `end` es el índice del `Message::assistant` que contiene la llamada a `fold`. Se elimina `staged[k..end]`.
4. Se inserta en `k` un único `Message::user_text` con el bloque de fold.

El fold se aplica **después** de construir el `Message::user_blocks(results)` de esa ronda, de modo que el modelo puede leer algo y replegarlo en la misma ronda.

Que `end` excluya el turno en vuelo no es solo por el emparejamiento: es también lo que preserva el thinking de ese turno, que es el único que la API exige de vuelta intacto.

Validación antes de aceptar: cada `tool_use` de cada assistant superviviente tiene su `tool_result`. Si falla, el fold se rechaza y devuelve `tool_result` con `is_error: true`. Es un evento conversacional normal, no un fallo del harness.

**El punto donde una implementación ingenua falla primero.** Un `retain[]` que apunta a un `tool-result` no puede re-emitirse como bloque `tool_result`: su `tool_use` se quedó en el archivo y la API rechazaría la petición. Se re-emite como texto dentro de `<attachments>` del bloque de fold. Cae del formato que propone la especificación, pero conviene dejarlo escrito porque es el error silencioso más probable.

## Thinking en la implementación

Tres reglas, en orden de aplicación. La primera es solo para `AnthropicProvider`; las otras dos valen para los tres.

**En el contexto vivo, el thinking de los turnos que sobreviven al fold se descarta del lado del modelo, no del nuestro.** Se envía `thinking.block_binding.prefix_mismatch_behavior = "drop_block"` bajo el beta header `thinking-binding-controls-2026-08-01`. La API descarta los bloques cuyo prefijo ya no coincide, la petición sale adelante, y la respuesta trae un array `input_transformations` con lo que descartó. Quitarlos nosotros del medio del historial sería justamente la fila "Inválido para todos los posteriores" de la tabla.

Esto implica cambios en `anthropic/types.rs`: `Thinking` gana un campo opcional `block_binding`, y `AnthropicProvider` añade el beta header. La condición es el **provider**, no el modelo: `codex_oauth.rs` y `openai_compatible.rs` construyen su propio cuerpo y ni siquiera miran `Thinking`, así que ahí no hay nada que enviar ni nada que romper. En esas dos rutas el fold no tiene lado de thinking en absoluto.

**En el archivo del fold, el thinking no se escribe.** El archivo es documentación para relectura, `<model id= tokens=>` solo lleva texto y llamadas a tools, tal como muestra `fold.md`. Nada lo va a reproducir contra la API, así que no hay firma que preservar.

**El turno en vuelo conserva su thinking íntegro**, por la elección de `end` descrita arriba. No hay que hacer nada, pero sí hay que fijarlo con un test: es el invariante que rompe un refactor futuro que decida "simplificar" el rango.

## Archivo del fold

Ruta: `.zest/folds/<thread-id>/<YYYYMMDDTHHMMSS>-<slug>.xml`.

No se reutiliza `SpillStore`. Su barrido (7 días, 64 MB, 64 ficheros) es correcto para artefactos desechables, y un fold no lo es: el bloque en contexto apunta al archivo y ese bloque persiste en `Thread::agent_messages`. Un fold barrido deja un puntero muerto permanente. `FoldStore` propio, con la misma validación de nombre que `SpillStore::write`, con `fsync` (aquí sí es estado durable) y borrado ligado al borrado del hilo, junto a `spill::remove_thread_dir`.

**Redacción.** El contenido se escribe pasando el rango por `redact_sensitive_staged` con `sensitive_tool_ids`, igual que `messages_for_persist`. Sin esto un fold escribe en claro un cuerpo que la persistencia normal redacta. Y un `retain[]` sobre un id marcado sensible se rechaza con error al modelo.

## Reinyección

El hueco del rango lo ocupa un `Message::user_text` con:

```
<fold id="…" slug="…" source=".zest/folds/t-abc/20260906T195527-slug.xml">
<summary>…</summary>
<attachments>
<user_query id="…" tokens="19">…</user_query>
<tool-result id="…" tokens="180">…</tool-result>
</attachments>
</fold>
```

Una desviación deliberada respecto de `fold.md`: sin envoltorio `<system>`. El system prompt real de Zest es `SystemPrompt` y viaja en `TurnRequest.system`. Etiquetar un mensaje de usuario como `<system>` le da al modelo una señal de autoridad que ese contenido no tiene, y Anthropic ya expone un canal real para eso (mensajes `role: "system"` dentro de `messages`, disponible en Opus 5 sin beta header) si alguna vez hace falta.

No hace falta maquinaria de reinyección diferida: en Zest el fold ocurre a mitad de turno y el bloque ocupa directamente el hueco, lo que es equivalente al "después del user_query" de la especificación.

## Recuperación y fold recursivo

Nada nuevo. `read_file` y `grep` ya alcanzan `.zest/`, que es como funciona `spill` hoy. El atributo `source=` es el localizador.

Límite a declarar: `read_file::MAX_BYTES` son 256 KiB. Un fold grande puede excederlo, igual que ya le pasa a spill, que emite un aviso condicional en `notice()`. El bloque de fold debe emitir el mismo aviso cuando el archivo supere ese alcance.

El fold recursivo cae del diseño: leer el XML de un fold es una tool call normal, y el fold siguiente incluye esa lectura en su rango.

## Integración con lo que ya existe

- **`compact_context` sigue siendo la red a 80%.** Fold no la sustituye: `AUTO_COMPACT_THRESHOLD_PERCENT` es un backstop del harness, fold es una decisión del agente.
- **Riesgo concreto:** la rama `Summarized` reemplaza `self.messages` por dos mensajes y con ello **pierde los punteros a todos los archivos de fold anteriores**. Hay que recolectar los bloques de fold antes de resumir y reemitirlos junto al checkpoint, o como mínimo listar sus `source=`.
- **La rama `Pruned` necesita `drop_block` igualmente**, por lo dicho en la sección de thinking. Es un arreglo previo a fold, no una consecuencia suya.
- **Tras aplicar un fold**, lo mismo que hace `compact_context`: `provider_session = None` (una sesión del provider espeja una historia que ya no existe) y `last_usage = None` (esa medición describe un prompt que ya no existe). Como el fold ocurre a mitad de turno y `provider_session` se reasigna desde `completion.provider_session` en cada ronda, hay que anularlo justo después de aplicar y antes de la ronda siguiente.
- **`mark_conversation_prefix`** puede seguir igual: añadir y mover marcadores `cache_control` es "Válido" en la tabla de edits, y `markable()` ya se niega a tocar bloques de thinking.
- **Delegación.** `tools.update_context(&handoff_messages)` pasa el contexto a los workers. Tras un fold ven el contexto replegado, que es lo deseado.

## Coste de caché

Fold reescribe el frente de `messages`. Todo lo posterior al punto de fold se paga como cache write en la ronda siguiente (1.25x, o 2x bajo `long_cache_control`). Un fold que ahorra 50k tokens y obliga a reescribir 20k de prefijo compensa con holgura; un modelo que hace fold cada dos rondas es una regresión neta.

La documentación confirma el mecanismo desde el otro lado: "A thinking block the API drops under either preserved-thinking condition changes the cached prefix from that block's position onward". Es decir, el `drop_block` que necesitamos tiene el mismo efecto sobre la caché que el propio fold. No se suman: el fold ya invalidó desde ese punto.

Guardarraíles mínimos:

- `MIN_FOLD_CONVERSATION_TOKENS`, análogo a `MIN_COMPACTION_CONVERSATION_TOKENS = 4_000`: rechazar folds cuyo rango, estimado con `context_budget:: conversation_tokens` sobre el sub-slice, no llegue al mínimo.
- Devolver en el `tool_result` del fold el ahorro real estimado (tokens fuera, tokens retenidos) para que el modelo calibre.
- Instrumentar folds por turno y ahorro acumulado. Sin medición no se sabe si paga.

## UI

`StoredMessage` es transcript separado de la historia de wire, y un fold no debe borrar nada de él: la persona sigue viendo su conversación entera. Mínimo: una fila de marcador con el slug y el enlace al archivo. Encaja como variante nueva de `ThreadEventKind` (`Folded { turn_id, slug, source, from_id, tokens_saved }`), siguiendo el patrón de `ToolCalled` / `ToolResult`, que ya está pensado para replay y reparación tras crash.

## Plan de implementación

### Fase 0, arreglo previo (solo Anthropic)

1. Añadir `block_binding` a `Thinking` en `anthropic/types.rs` y el beta header `thinking-binding-controls-2026-08-01` en `AnthropicProvider`, condicionados al provider. Aplicarlo ya a la rama `Pruned` de `compact_context`, que hoy es un fallo latente independiente de fold: reescribe cuerpos de `tool_result` anteriores y conserva el thinking posterior.
2. Hacer que `redact_sensitive_staged` conserve el sobre `<tool-result …>` y sustituya solo el cuerpo. Va antes que la fase 2 porque es lo que hace que el direccionamiento sobreviva a una recarga de hilo.

### Fase 1, el módulo

3. `crates/core/src/fold.rs`: `entry_id()`, render del marcado incremental, render del bloque de fold, parser de cabecera, `FoldStore` con validación de nombre. Tests puros.

### Fase 2, direccionamiento

4. Emitir el marcado en `send_user_cancellable`, siempre dentro del payload de texto del primer y último bloque del mensaje de usuario: `</model>` y `<model-metadata/>` al principio, el sobre `<user_query>` o `<tool-result>` alrededor del contenido, y `<model>` al final. Nunca como bloques `text` hermanos de un `tool_result`, por lo dicho en la sección de serialización. Y no en los constructores de `Message`: `compact_context` y `provider::probe` también los usan y no deben llevar marcado.
5. `fn entry_index(messages: &[Message], id: &str) -> Option<usize>`.
6. Tests: la recarga del hilo conserva los IDs, el prune conserva las etiquetas de apertura y cierre del `tool-result`, la redacción sensible se comporta igual.

### Fase 3, la operación

7. `FOLD_TOOL` como fachada `Tool`, registrado en `runtime.rs` bajo `!provider_owns_agent_loop && is_parent`, junto a donde ya se condicionan `ask_user` y `browser`.
8. Intercepción con slot exclusivo en `execute_tool_calls`, aplicación en `send_user_cancellable`.
9. `apply_fold(staged, request) -> Result<FoldReport, String>`: resolución de `from`, ajuste a frontera de turno, validación de emparejamiento, escritura del archivo redactado y sin thinking, sustitución del rango.
10. Anular `provider_session` y `last_usage`.
11. Test que fija el invariante: tras un fold, el último `Message::assistant` conserva sus bloques de thinking intactos.

### Fase 4, integración

12. Preservar los bloques de fold en `compact_context`.
13. `ThreadEventKind::Folded` y fila de marcador en la UI.
14. Borrado de `.zest/folds/<thread-id>/` junto al borrado del hilo.

### Fase 5, guardarraíles

15. `MIN_FOLD_CONVERSATION_TOKENS`, ahorro reportado en el resultado, instrumentación, y lectura de `input_transformations` para registrar cuántos bloques descartó la API.
16. Descripción en el system prompt de cuándo usar `fold`. Sin esto el modelo no lo usa, o lo usa mal.

### Opcional

- Parámetro `to` para acotar el final del rango.
- `retain` parcial por rangos de líneas.
- Índice `.zest/folds/<thread-id>/index.xml` para listar folds sin leerlos.
- Fold automático disparado por el harness al 80% en lugar de `compact_context`, una vez haya medición que lo respalde.

### Diferido

- Fold en providers con `owns_agent_loop() == true`.
- Marcado XML en los turnos del modelo mismos, que seguiría siendo manipulación de su salida y no aporta nada que `model-metadata` no dé ya.

## Riesgos

| Riesgo | Modo de fallo | Mitigación |
| --- | --- | --- |
| Thinking inválido tras el fold (solo Anthropic) | 400 en cuentas nuevas y en Fable 5.1; se generalizará | `prefix_mismatch_behavior: "drop_block"` |
| `tool_result` retenido re-emitido como bloque | 400 en la ronda siguiente | Re-emitir siempre como texto en `<attachments>` |
| `from` a mitad de una ronda de tools | `tool_use` huérfano, 400 | Ajuste a frontera de turno más validación previa al commit |
| `end` incluye el turno en vuelo | Se pierde el thinking que la API sí exige, 400 | `end` excluye ese mensaje, fijado con test |
| Compactación tras folds | Punteros a archivos perdidos para siempre | Preservar bloques de fold en `compact_context` |
| Barrido del archivo de fold | Puntero muerto permanente en historia persistida | `FoldStore` propio sin barrido por edad |
| Cuerpo sensible en el archivo | Copia en claro de algo que la persistencia redacta | `redact_sensitive_staged` antes de escribir, rechazo de `retain` sensible |
| Fold demasiado frecuente | Regresión neta de coste por cache write | `MIN_FOLD_CONVERSATION_TOKENS` más ahorro reportado |
| Colisión de IDs en batch paralelo | Dos elementos direccionables iguales | Desempate por `ID_SEQ` |
| `model-metadata` perdido en Codex y OpenAI-compatible | Los turnos del modelo se quedan sin ID, `from` y `retain` no pueden apuntar a ellos | Todo el marcado dentro del `content` del `tool_result`, nunca como bloque `text` hermano |
| Resultado sensible redactado | Se pierde el ID del resultado y el `model-metadata` anterior al reabrir el hilo | `redact_sensitive_staged` conserva el sobre y sustituye solo el cuerpo |
