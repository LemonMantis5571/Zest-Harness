# Migración a Rig

Decidido: migrar la capa de providers nativos a `rig-core`, con `codex_oauth` como prioridad. Este documento fija el alcance, la forma de la migración y el orden.

## Alcance

Tres providers nativos migran a Rig:

| Zest hoy | Rig | Prioridad |
| --- | --- | --- |
| `codex_oauth` (suscripción ChatGPT, backend Responses) | `providers::chatgpt` | 1 |
| `openai_compatible` (OpenAI API y endpoints compatibles) | `providers::openai` con `base_url` propio | 2 |
| `anthropic` | `providers::anthropic` | No es prioridad |

Anthropic por API apenas se usa: el camino real hacia Claude es el puente CLI `claude_code`, que no se migra. La fase 3 queda como opcional y no bloquea nada. Esto simplifica el riesgo principal de la migración, porque las firmas de thinking y `block_binding` viven solo en esa ruta.

Tres puentes CLI quedan **congelados, no borrados**: `claude_code`, `codex_cli`, `cursor_acp`. Siguen compilando y funcionando tal como están. No se migran porque `owns_agent_loop() == true` y corren su propio bucle sobre stdio, así que Rig no aporta nada ahí. Se marcan como ruta congelada en `driver.rs` y en `zest.toml.example`, sin retirar código ni romper configuraciones existentes.

El default de `zest.toml.example` se queda en `kind = "codex_cli"`. Cambiarlo a `codex_oauth` es un cambio visible para cualquiera que copie el ejemplo y no hace falta para migrar: `codex_oauth` se configura igual que hoy y la migración es interna al provider. Queda fuera de este stage junto con el resto de `codex_cli`.

`openai_compatible` conserva su carácter genérico. `rig::client::Client` guarda `base_url` como campo propio y su documentación contempla explícitamente base URLs vacías para que el usuario ponga la suya, así que DeepSeek y cualquier otro endpoint siguen funcionando con la misma entrada de config que hoy.

## Modelos: el catálogo de Zest manda

`completion_model(client, model: String)` de Rig acepta cualquier string, y sus constantes son solo conveniencia. Están además desactualizadas respecto a Zest: Rig expone `gpt-5.3-codex`, `gpt-5.3-codex-spark`, `gpt-5.4`, `gpt-5.4-pro`, mientras que `CODEX_KNOWN_MODELS` es `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna`, `gpt-5.5`, `gpt-5.4`, `gpt-5.4-mini`, con `gpt-5.6-sol` por defecto.

Regla para toda la migración: **no adoptar las constantes de Rig**. `CODEX_KNOWN_MODELS`, `cursor_models::BUILTIN_MODELS`, `catalogue()`, `EffortPolicy` y `context_window_for_model` siguen siendo la fuente de verdad, y el id de modelo se pasa a Rig como string. `validate_selection` sigue rechazando pares modelo/effort desconocidos antes de gastar cuota, igual que hoy.

## Por qué no el funnel OpenAI-compatible

Se descartó enrutar todo por OpenAI-compatible. Queda anotado porque explica por qué cada provider se migra nativo en lugar de colapsarlos en uno.

Falla en dos niveles. Primero, los tres puentes CLI no son endpoints HTTP: son procesos con stdio, no hay `base_url` al que apuntar. Segundo, en Anthropic la capa de compatibilidad descarta justo lo que Zest usa, según su propia documentación:

| Lo que Zest usa | Estado en la capa OpenAI-compatible |
| --- | --- |
| `cache_control`, `long_cache_control` (TTL 1h), breakpoints rodantes | "Prompt caching is not supported" |
| `Usage::prompt_tokens()` sumando las dos columnas de caché | `usage.prompt_tokens_details`: "Always empty" |
| `output_config.effort` | `reasoning_effort`: "Ignored" |
| Bloques `thinking` con `signature` | No se devuelven |
| `SystemPrompt` partido en cacheable y volátil | Los mensajes de sistema se izan y concatenan |

Más el encabezado de la página: "not considered a long-term or production-ready solution for most use cases". Rig da el mismo beneficio de mantenibilidad con cada provider nativo y sin esa pérdida.

## Forma: la capa de providers, no el bucle

`CompletionModel` en Rig es un trait con `completion()` y `stream()`, usable sin su abstracción `Agent`. Esa es la línea de corte.

**Se migra:** `provider/anthropic.rs`, `provider/openai_compatible.rs`, `provider/codex_oauth.rs`, y con ellos `anthropic/client.rs`, `anthropic/sse.rs` y `anthropic/accumulate.rs`.

**No se migra `agent.rs`.** El bucle es el producto: staging transaccional, gating con `ApprovalPolicy`, `spill`, checkpoints, ledger de delegación, `ask_user`, cancelación cooperativa. Nada de eso tiene equivalente en `rig::Agent`, y sustituirlo sería tirar el harness para quedarse con el cliente HTTP.

El trait `Provider` de Zest se queda como está. `TurnRequest`, `Completion` y `StreamEvent` no cambian. Cada provider nativo pasa a traducir `TurnRequest` a un `CompletionRequest` de Rig y a mapear el stream de vuelta a `StreamEvent`. `agent.rs` no se entera de la migración.

## Lo que hay que resolver

**El round-trip sin pérdida decide hasta dónde llega la migración.** `anthropic/types.rs` documenta por qué `Message.content` es `Vec<serde_json::Value>`: la API añade tipos de bloque con el tiempo (`server_tool_use`, `fallback`) y un enum tipado descarta en silencio lo que no modela. Rig usa enum tipado. Según la documentación de preserved thinking, descartar o alterar un bloque en medio del historial invalida todos los bloques de thinking posteriores, así que una pérdida silenciosa no da un error legible sino un 400 varios turnos después.

Prueba de aceptación, no discusión: un test que meta en el historial un bloque de un tipo que Rig no modele, lo pase por la traducción de ida y vuelta y compare bytes. Si no sobrevive, la salida es `additional_params` (que es `Option<serde_json::Value>` en `CompletionRequest`) para transportar los bloques crudos. Si tampoco basta, la fase 3 no se hace y Anthropic se queda nativo.

**El almacén de credenciales de Codex.** Rig hace refresh OAuth completo con margen de expiración y persiste el resultado, pero escribe a un fichero de auth. Zest guarda la sesión en el gestor de credenciales del sistema vía `CodexOAuthSession` y `refresh_and_store`. El campo es `Option`, así que hay una vía sin fichero, pero es el punto de integración concreto de la fase 1 y conviene mirarlo antes de empezar.

**La colocación de breakpoints de caché.** `mark_conversation_prefix` pone dos breakpoints rodantes en posiciones concretas, con un razonamiento explícito sobre la ventana de lookback de 20 bloques y las rondas de tools en abanico. Hay que comprobar que Rig permite esa colocación exacta, no solo que "soporta cache_control". Si solo ofrece marcado automático al final, se pierde el ahorro en los turnos más caros. Es un requisito de la fase 3.

**Cobertura de `ModelSpec` y `EffortPolicy`.** El catálogo valida pares modelo/effort antes de gastar cuota, y `context_window_for_model` alimenta el medidor. Hay que ver qué da `model_listing` de Rig y qué se sigue manteniendo aparte.

## Fases

**Fase 0, hecha.** `provider/rig_convert.rs` con la traducción entre `Vec<Message>` y el modelo de Rig, 9 tests, suite completa en verde (898 pasan). Resultado del spike, que es lo que había que averiguar:

`AssistantContent` de Rig tiene cuatro variantes (`Text`, `ToolCall`, `Reasoning`, `Image`) y **no tiene passthrough**, así que un `server_tool_use` o un `fallback` de Anthropic no tienen dónde ir. La conversión devuelve `ConvertError` nombrando el tipo de bloque en lugar de descartarlo, que es la única salida honesta: un descarte silencioso da un 400 varios turnos después sin nada que apunte aquí. Para Codex y OpenAI no aplica, porque esos bloques no aparecen en sus rutas. Para Anthropic significa que la fase 3, si algún día se hace, necesita `additional_params` como transporte de bloques crudos.

Lo que sí round-trip bien y está cubierto por tests: texto, `tool_use` y `tool_result` con el id del provider en las dos mitades (`wire_call_id`), `thinking` con su `signature` incluso cuando el texto viene vacío por `display: "omitted"`, `redacted_thinking`, y el izado de contenido de sistema a instrucciones.

**Fase 1, Codex: código hecho, falta validación contra el backend real.** `provider/codex_rig.rs` sirve el turno por `providers::chatgpt` y `CodexOAuthProvider::stream_turn` ya llama ahí. Workspace compila limpio, 900 tests en verde.

El almacén de credenciales se resolvió sin decisión pendiente: `ChatGPTAuth::AccessToken { access_token, account_id }` acepta un token ya válido, así que Zest conserva su keyring y su `refresh_and_store` y a Rig solo le llega el token. No hay segundo almacén ni fichero de auth.

El camino artesanal (`stream_responses`, `ResponsesAccumulator`, `send_once`, `chatgpt_stream_error`) queda como ruta muerta marcada con `#[allow(dead_code, reason = …)]`, no borrado. Sus tests siguen ejecutándose y siguen guardando la forma del wire, que es útil como referencia mientras la ruta nueva no esté validada en vivo.

Lo que **no** está hecho y hay que hacer antes de considerar la fase cerrada:

- Probar un turno real contra el backend. Todo lo anterior es compilación y tests unitarios; el mapeo de eventos, el `finish_reason` y el usage solo se confirman con una respuesta de verdad.
- Comprobar si el turno reintenta. El camino viejo tenía `MAX_ATTEMPTS = 3`, `CONNECT_TIMEOUT` y `STREAM_IDLE_TIMEOUT` propios. Rig trae los suyos y no los he verificado, así que hoy es una regresión potencial de robustez, no confirmada en ninguna dirección.
- `Include::ReasoningEncryptedContent` lo añade Rig por su cuenta, así que la ruta empieza a devolver razonamiento cifrado. `from_rig_content` ya lo persiste como bloque `reasoning_encrypted` y lo lee de vuelta, con test de ida y vuelta, pero conviene mirar cuánto contexto ocupa en un turno real.

**Fase 2, OpenAI API.** `OpenAiCompatibleProvider` sobre `providers::openai` con `base_url` configurable. Sin firmas y sin caché de Anthropic, es el provider de menor riesgo; sirve para consolidar la forma de traducción que la fase 1 estrenó.

**Fase 3, Anthropic.** Solo si la fase 0 salió limpia. Es el provider con más superficie: caché, thinking, `block_binding`, redacción y ledger.

**Fase 4, limpieza.** Retirar `anthropic/client.rs`, `sse.rs` y `accumulate.rs` cuando ningún provider los use. No antes, y como paso separado.

**Independiente de todo lo anterior:** `rmcp`, el SDK oficial de MCP, contra las 2469 líneas de `mcp.rs` y `mcp/http.rs`. No toca ni el bucle ni el diseño de fold.

## Qué le deja pendiente a fold

Fold queda fuera de este stage y se implementa cuando la migración termine. `fold` opera sobre `Vec<Message>` en `agent.rs`, que es justo lo que no se migra, así que estructuralmente no hay conflicto: al terminar seguirá habiendo un `Vec<Message>` y un `Provider` con la misma forma.

Lo que sí cambia es una premisa concreta de `context-folding.md`, y hay que arrastrarla para no implementar contra un documento caduco. Ese plan dice que la ruta Codex hoy no manda razonamiento, y de ahí concluye que ahí un fold es un reemplazo de array de mensajes sin lado de thinking. **La fase 1 lo invalida**, porque Rig fuerza `Include::ReasoningEncryptedContent` y la ruta pasa a llevar items de reasoning. Cuando se retome fold habrá que decidir si los items del rango replegado se descartan igual que en Anthropic y reescribir la sección de alcance de ese plan.

Entrega de este stage, además del código: `context-folding.md` revisado contra la superficie post-migración. Es lo último de la fase 4, no de la fase 1, para que refleje el estado final de los tres providers.
