<p align="center">
  <a href="https://github.com/pacolang/paco"><img alt="License" src="https://img.shields.io/github/license/pacolang/paco"></a>
  <a href="https://github.com/pacolang/paco/stargazers"><img alt="Stars" src="https://img.shields.io/github/stars/pacolang/paco"></a>
  <a href="https://github.com/pacolang/paco/network/members"><img alt="Forks" src="https://img.shields.io/github/forks/pacolang/paco"></a>
  <a href="https://github.com/pacolang/paco/issues"><img alt="Issues" src="https://img.shields.io/github/issues/pacolang/paco"></a>
</p>

<h1 align="center">Paco</h1>

<p align="center">
  Un lenguaje de programación de propósito general y compilado, para construir sistemas de IA de extremo a extremo — servicios, CLIs, juegos, flujos críticos y compiladores sobre una misma base, en un único binario.
  <br />
  <a href="docs/design/spec.md"><strong>Explora la especificación »</strong></a>
  <br />
  <br />
  <a href="https://github.com/pacolang/examples">Ver Ejemplos</a>
  ·
  <a href="https://github.com/pacolang/paco/issues/new">Reportar un Error</a>
  ·
  <a href="https://github.com/pacolang/rfcs">Proponer una RFC</a>
</p>

**Leer en:** [English](README.md) · [Português](README.pt-BR.md) · **Español**

> **Estado:** implementación temprana. El compilador verifica, ejecuta (`paco run`, compilación cacheada con Cranelift) y compila a binarios nativos vía Cranelift y LLVM (`paco build`); la biblioteca estándar todavía es mínima.

## Índice

- [Acerca de](#acerca-de)
- [Principios](#principios)
- [Ecosistema](#ecosistema)
- [Primeros Pasos](#primeros-pasos)
- [Uso](#uso)
- [Estructura del Repositorio](#estructura-del-repositorio)
- [Hoja de ruta](#hoja-de-ruta)
- [Contribuir](#contribuir)
- [Licencia](#licencia)
- [Contacto](#contacto)

## Acerca de

Servicios, CLIs, juegos, flujos críticos, compiladores y sistemas de IA comparten una misma base; construir sistemas de IA de extremo a extremo es el criterio de desempate cuando dos diseños entran en conflicto ([RFC 0013](https://github.com/pacolang/rfcs/blob/main/text/0013-ai-systems-north-star.md), [RFC 0028](https://github.com/pacolang/rfcs/blob/main/text/0028-general-purpose-ai-tie-breaker.md)).

Paco está en la etapa de diseño/bootstrap. `AGENTS.md` es la referencia rápida del lenguaje; `docs/design/spec.md` es la especificación completa (el "qué"); `docs/implementation/requirements.md` es la hoja de ruta (el "cómo construirlo").

## Principios

1. **Con criterio propio, pero con libertad** — una forma recomendada, con escapes explícitos.
2. **Costo visible** — sin asignación, copia ni comportamiento dinámico ocultos.
3. **Bajo costo mental por defecto** — la complejidad solo aparece cuando la necesitas.

## Ecosistema

Una organización, `github.com/pacolang`, un repositorio por proyecto. Este repositorio, `pacolang/paco`, es el núcleo: compilador, runtime y `stdlib`, una sola versión. Cada biblioteca de dominio oficial — `pacolang/tensor`, `pacolang/math`, `pacolang/blas`, y otras según se propongan — obtiene su propio repositorio, su propia versión semántica, y su propia declaración de rango de versión del compilador en su `paco.mod`; ninguna de ellas se distribuye junto con este repositorio. Un programa obtiene una con `paco get github.com/pacolang/<nombre>@<versión>`, de la misma forma que obtiene cualquier otra dependencia. `docs/ecosystem.md` tiene los criterios de admisión, las reglas de dependencia y la lista completa actual ([RFC 0030](https://github.com/pacolang/rfcs/blob/main/text/0030-repository-organization-and-stdlib-scope.md)).

`stdlib/numerics.paco`, `stdlib/math.paco` y `stdlib/blas.paco` (`Tensor`, `Matrix`, `DataFrame` y el binding de BLAS) todavía viven en este repositorio hoy — están marcados para su extracción a `pacolang/tensor`, `pacolang/math` y `pacolang/blas` respectivamente, en un cambio futuro y dependiente.

Los programas de ejemplo viven en su propio repositorio, [`pacolang/examples`](https://github.com/pacolang/examples) — no aquí.

## Primeros Pasos

### Requisitos previos

- El toolchain de Rust fijado en `rust-toolchain.toml`.
- LLVM 18.1 para el backend optimizador (ver `compiler/paco-codegen-llvm/README.md`). Sin él, `cargo build --no-default-features -p paco-driver` compila un `paco` cuyo `run` y `build` usan solo Cranelift.
- macOS: las herramientas de línea de comandos de Xcode (`xcode-select --install`).
- Windows: las MSVC build tools y el Windows SDK.

### Instalación

Paco compila y funciona en Linux (`x86_64`, `aarch64`), macOS (`aarch64`, `x86_64`) y Windows (`x86_64`). En Linux, `paco build` no necesita nada más que los requisitos previos anteriores (`scripts/dist.sh` empaqueta una distribución autónoma).

```bash
cargo build --release
```

## Uso

```bash
paco new hello               # crea un proyecto
paco run                     # compila (con caché) + ejecuta
paco build --release         # backend optimizador, binario único
paco test                    # ejecuta funciones #[test]
paco fmt --write             # formateador canónico
```

Consulta [`pacolang/examples`](https://github.com/pacolang/examples) para programas completos y ejecutables.

## Estructura del Repositorio

```
paco/
├── AGENTS.md            # contexto para agentes de IA
├── README.md
├── docs/
│   ├── design/          # la especificación del lenguaje
│   ├── implementation/  # requisitos + hoja de ruta
│   └── grammar/         # tokens y gramática EBNF
├── tests/conformance/   # pruebas: input.paco + salida esperada, ejecutadas en cada backend
├── compiler/            # RUST — el compilador
├── runtime/             # RUST — scheduler, channels, I/O poller (enlazado en los binarios)
└── stdlib/              # PACO — la biblioteca estándar
    ├── core/            #   prelude: Option, Result, traits, collections, derive
    ├── io.paco, string.paco, sync.paco, collections.paco, dims.paco, autodiff.paco
    └── math.paco, numerics.paco, blas.paco   # marcados para extracción, ver docs/ecosystem.md
```

La frontera Rust ↔ Paco: lo que se ejecuta **debajo** del lenguaje (compilador, runtime) es Rust; lo que vive **dentro** del lenguaje (biblioteca estándar) es Paco.

## Hoja de ruta

Consulta `docs/implementation/requirements.md` para el plan de construcción, y las [issues](https://github.com/pacolang/paco/issues) y [milestones](https://github.com/pacolang/paco/milestones) de este repositorio para el seguimiento del día a día.

## Contribuir

Las contribuciones son lo que hace que la comunidad de código abierto sea un lugar increíble para aprender y crear. Cualquier contribución tuya es **muy bienvenida**.

1. Haz un fork del repositorio.
2. Crea tu rama de feature (`git checkout -b feat/mi-feature`).
3. Lee `AGENTS.md` antes de tocar cualquier archivo `.paco` o el código del compilador.
4. Haz commit de tus cambios y abre un pull request.

Para un cambio de diseño de lenguaje o de ecosistema, abre primero una RFC en [`pacolang/rfcs`](https://github.com/pacolang/rfcs).

## Licencia

Distribuido bajo la Apache License, Version 2.0. Consulta [`LICENSE`](LICENSE) para más información. Los programas construidos con Paco pueden publicarse bajo cualquier licencia.

## Contacto

Enlace del proyecto: [https://github.com/pacolang/paco](https://github.com/pacolang/paco)
