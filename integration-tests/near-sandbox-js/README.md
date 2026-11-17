# ROASTAKE — NEAR Sandbox Integration Test Harness (JavaScript)

Arnés aislado de pruebas de integración para contratos inteligentes de **ROASTAKE** utilizando `near-workspaces` (JavaScript / Node.js) sobre un entorno local de **NEAR Sandbox**.

---

## 1. ¿Por qué este arnés es independiente de las pruebas unitarias de Rust?

Las **103** pruebas unitarias en Rust del workspace (`88` en `contracts/oracle-adapter/src/tests.rs`, `5` en `workers/coffee-price-index`, `10` en `packages/shared`) se ejecutan sobre el simulador `near_sdk::testing_env!`. Dicho simulador es puramente síncrono y en memoria:
- No programa recibos asíncronos reales a través de shards.
- No sufre retrasos de red ni límites reales de gas de bloques.
- No ejecuta el ciclo de vida nativo de promesas `promise_yield_create` y `promise_yield_resume` (NEP-519) con timeouts reales de 200 bloques.

Este arnés de integración levanta un nodo univalidador completo de `nearcore` en modo **Sandbox**, permitiendo desplegar los binarios WASM compilados reales, crear cuentas con transacciones reales y verificar recibos y saldos en condiciones de runtime auténticas.

---

## 2. ¿Por qué se ejecuta bajo WSL2 y no en Windows nativo?

El paquete oficial `near-sandbox` descarga y ejecuta el binario del nodo univalidador de NEAR (`near-sandbox`). El repositorio oficial de `nearcore` publica binarios precompilados exclusivamente para arquitecturas **Linux** (`x86_64`, `aarch64`) y **macOS** (`arm64`). La ejecución nativa en Windows (`Windows_NT`) no está soportada por el descargador de binarios de `near-sandbox` y genera una excepción fatal.

Por ello, este arnés opera en **Ubuntu WSL2**, utilizando la toolchain nativa de Linux (Node.js 24 + pnpm 10).

---

## 3. Acceso y Ejecución desde Ubuntu WSL2

### 3.1 Ingresar a Ubuntu
Desde una terminal de Windows PowerShell:
```powershell
wsl -d Ubuntu
```

### 3.2 Navegar al Repositorio
El sistema de archivos de Windows está montado en `/mnt/c/`:
```bash
cd /mnt/c/Users/M/Documents/Roastake/integration-tests/near-sandbox-js
```

### 3.3 Verificar Toolchain de Linux
Asegurarse de que `node` y `pnpm` apunten a los binarios de Linux bajo `/home/m/.nvm/`:
```bash
node -v   # v24.22.2
pnpm -v   # 10.33.2
```

### 3.4 Instalar Dependencias
Instalar únicamente dentro del paquete aislado:
```bash
pnpm install
```

---

## 4. Ejecución de Pruebas

> [!IMPORTANT]
> **Protección de Red Absoluta:** Este arnés está configurado para forzar de forma estricta y exclusiva el modo `sandbox` (`NEAR_WORKSPACES_NETWORK=sandbox`). Si se detecta cualquier variable que intente apuntar a `testnet` o `mainnet`, la ejecución aborta inmediatamente (*fail-closed*). Nunca utiliza credenciales reales de billeteras ni interactúa con redes públicas.

### Ejecutar Smoke Test
```bash
pnpm test:smoke
```

### Ejecutar Toda la Suite de Pruebas
```bash
pnpm test
```

---

## 5. Ciclo de Vida y Cierre Limpio del Sandbox

Cada prueba instancia un `Worker` de `near-workspaces` que arranca el proceso `near-sandbox` en un directorio temporal aislado. El bloque `finally` de las pruebas garantiza que `await worker.tearDown()` sea invocado incondicionalmente, deteniendo el proceso del servidor RPC local y liberando los puertos y archivos temporales incluso si una aserción falla.

---

## 6. Escenarios E2E Implementados y Verificados (Misiones 03C.3.6 – 03C.3.7.6)

El arnés ejecuta la suite completa de integración (`pnpm test:e2e`, `pnpm test:hardening` y `pnpm test:release-candidate`; **23** pruebas hoja distintas en total: **21** sobre el candidato a lanzamiento, **1** `test:smoke` sin contrato adaptador y **1** `test:compat` sobre el artefacto legado M03C.3.2; Node reporta **25** entradas al incluir 2 contenedores de suite) sobre `nearcore 2.14.0` (protocolo 88) desplegando `roastake_oracle_adapter_release_candidate_m03376.wasm` (`378,335 bytes`, SHA-256 `a9a07710ed10133b36b503fe81185cd83a8bbc2179a3f17ce8f09cc94c3756b8`) y el mock alineado `mock_outlayer_abi_fixed.wasm` (`142,932 bytes`, SHA-256 `a37bd40d21a7d85e0de6e6bfb369bebe41313222dd938b6d6a5361d1d99883e4`). Las pruebas locales con `Mock OutLayer` **no** demuestran ejecución en vivo contra `outlayer.testnet`.

1. **Test A — Ejecución Exitosa del Worker:** Almacena el precio validado (`1850000000`), limpia `pending_requests`, mantiene pasivo `0` y preserva el invariante formal de solvencia.
2. **Test B — Fallo Normal del Worker (`null`):** Emite `execution_failed` sin actualizar precios ni crear pasivos locales (OutLayer reembolsa directamente a `payer_account_id = Some(caller)`).
3. **Test C — Fallo del Recibo Inicial de OutLayer (`InitialPanic`):** NEAR devuelve el depósito al adaptador; se registra `UnreconciledFailure`, se rechaza la promoción prematura, el admin designa la reserva devuelta (`admin_designate_refund_reserve`), se promueve a `RefundCredit` (`promote_unreconciled_refund`) y el usuario reclama su reembolso.
4. **Test D — Fallo Downstream tras Aceptación de Depósito (`DownstreamPromiseFailure`):** Mock OutLayer retiene los `50 mNEAR`; el adaptador registra `UnreconciledFailure` con `0` pasivo creado, rechaza tanto `admin_designate_refund_reserve` como `promote_unreconciled_refund` por falta de fondos no asignados, y protege los créditos respaldados de otros usuarios.
5. **Test D2 — Reserva Explícita de Seguro (`fund_refund_reserve`):** El fondeo explícito de `refund_reserve_balance` permite promover de forma solvente un `UnreconciledFailure` downstream y liquidar su pago.
6. **Test E — Reclamo Exitoso y Limpieza Terminal:** Transfiere `50 mNEAR` al reclamante, reduce `total_refund_liability` una sola vez, elimina el registro terminal de `claims` (`get_claim == null`) e impide reclamos repetidos.
7. **Test F — Ciclo de Vida de Almacenamiento de Reclamos:** Verifica a través de múltiples ciclos de reclamo y liquidación que la eliminación de reclamos terminales produce `0 bytes` de crecimiento acumulado por reclamo en el trie de almacenamiento.
8. **Test G — Fallos Irreconciliados Concurrentes:** Dos fallos concurrentes con reserva fondeada para uno solo permiten promover únicamente el primero y rechazan el segundo (*fail-closed*).
9. **Test H — Transferencia de Reembolso Fallida y Retorno Diferido (`ReturnPending`):** Simula un fallo real de `Transfer` en Sandbox (`AccountDoesNotExist`), verifica la transición a `ReturnPending` sin pérdida de derecho ni consumo cruzado de respaldo, reconcilia mediante `reconcile_returned_refund` y completa el cobro único.
10. **Test I — Seguridad ante Callbacks Duplicados, Tardíos y Máquina de Estados de Resolución Externa (`L-02`):** Confirma que invocaciones directas a callbacks privados, promociones duplicadas, promociones bloqueadas mientras `external_resolution_proposed == true`, rechazo legítimo por el solicitante (`reject_unreconciled_external_resolution`) / cancelación admin (`admin_cancel_unreconciled_external_resolution`), resoluciones externas tras promoción y reconciliaciones administrativas posteriores a la liquidación fallan o transicionan de forma segura.

---

## 7. Suite Consolidada de Endurecimiento, Solvencia de Seguro y Candidato a Lanzamiento (`pnpm test:hardening`, Misiones 03C.3.7.4 – 03C.3.7.6)

La suite [`test/sandbox-admin-hardening.test.mjs`](file:///C:/Users/M/Documents/Roastake/integration-tests/near-sandbox-js/test/sandbox-admin-hardening.test.mjs) despliega `roastake_oracle_adapter_release_candidate_m03376.wasm` (`a9a07710ed10133b36b503fe81185cd83a8bbc2179a3f17ce8f09cc94c3756b8`) junto al mock alineado `mock_outlayer_abi_fixed.wasm` y verifica:

- **Test A (`Transfer` exitoso + callback ausente):** `ConfirmedFailure` tras `100` bloques mueve a `Disputed` (no pagable), bloqueando todo segundo cobro.
- **Test B (`Transfer` fallido + callback ausente + reembolso de sistema retrasado):** ni el estado `InFlight` ni el arribo posterior de fondos auto-restauran `Available` sin callback objetivo o seguro explícito.
- **Tests C y D (Aislamiento ante excedentes agrupados):** un excedente de 5 NEAR o reservas en `refund_reserve_balance` no pueden convertir un reclamo `Disputed` en `Available`, `ReturnPending` ni pago de seguro.
- **Tests E y F (Seguro separado y concurrencia):** `compensate_disputed_claim` permanece deshabilitado por defecto, requiere `fund_insurance_reserve` dedicado y ventana `DISPUTE_RESOLUTION_DELAY_BLOCKS` (1,000 bloques), asigna `compensation_id` independiente (límite por defecto `32`, techo duro `100`) e impide que dos reclamos concurrentes reutilicen la misma reserva.
- **Test G (Medición de almacenamiento, flush y límites):** verifica que la resolución externa propuesta no libera el slot hasta que el solicitante ejecuta `acknowledge_unreconciled_resolved_external`, y verifica el bloqueo exacto al alcanzar `max_open_requests` y al agotarse el margen libre de almacenamiento (< 512 bytes / `5.12 mNEAR`).
- **Test H (Propuestas administrativas no destructivas y llamadas tardías, `F-01`):** demuestra que `ConfirmedSuccess` solo registra `admin_settlement_proposed = true` sin eliminar el reclamo ni reducir pasivos hasta `acknowledge_disputed_transfer_received`, tras lo cual toda operación tardía falla cerrada.
- **Test I (Despliegue limpio vs incompatibilidad Borsh heredada):** demuestra en Sandbox vivo que actualizar WASM sobre estados legados `M03C.3.6` (`f4ae912d...`) o `M03C.3.7.2` (`bc5fe10d...`) sin migración falla de forma cerrada en la deserialización Borsh.
- **Test J (Liveness de reclamos ante `Disputed` histórico `F-03` e inyección sin duplicación `F-06`):** verifica que un reclamo histórico `Disputed` (`active_claim_id == null`) no bloquea el cobro de un nuevo `available_amount` independiente mediante `claim_refund()`.
- **Test K (Solvencia de seguro en vuelo, retorno fallido, contabilidad sin excedente fantasma `L-01` y recuperación de callback perdido `F-02`):** ejecuta transiciones reales (`claim_refund`, `compensate_disputed_claim`), verifica cero excedente fantasma (`+50 mNEAR` no duplicado), valida `ReturnPending` -> `reconcile_returned_insurance`, rechazo ante reserva insuficiente (`ERR_IMPOSSIBLE_INSURANCE_ACCOUNTING`), deducción exacta de `insurance_reserve_balance` y `account.amount`, y `mark_insurance_compensation_disputed` -> `acknowledge_insurance_compensation_received`.
- **Test L (Reserva operativa segregada `F-07` y rotación de admin en dos pasos `F-01`):** verifica que `fund_operating_reserve` incrementa `minimum_operating_reserve` impidiendo su reclasificación, que `set_admin` está deshabilitado y que `propose_admin` + `accept_admin` transfiere la administración de forma segura.
