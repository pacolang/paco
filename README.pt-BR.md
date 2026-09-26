<p align="center">
  <a href="https://github.com/pacolang/paco"><img alt="License" src="https://img.shields.io/github/license/pacolang/paco"></a>
  <a href="https://github.com/pacolang/paco/stargazers"><img alt="Stars" src="https://img.shields.io/github/stars/pacolang/paco"></a>
  <a href="https://github.com/pacolang/paco/network/members"><img alt="Forks" src="https://img.shields.io/github/forks/pacolang/paco"></a>
  <a href="https://github.com/pacolang/paco/issues"><img alt="Issues" src="https://img.shields.io/github/issues/pacolang/paco"></a>
</p>

<h1 align="center">Paco</h1>

<p align="center">
  Uma linguagem de programação de propósito geral e compilada, para construir sistemas de IA de ponta a ponta — serviços, CLIs, jogos, fluxos críticos e compiladores em uma mesma base, em um único binário.
  <br />
  <a href="docs/design/spec.md"><strong>Veja a especificação »</strong></a>
  <br />
  <br />
  <a href="https://github.com/pacolang/examples">Ver Exemplos</a>
  ·
  <a href="https://github.com/pacolang/paco/issues/new">Reportar um Bug</a>
  ·
  <a href="https://github.com/pacolang/rfcs">Propor uma RFC</a>
</p>

**Leia em:** [English](README.md) · **Português** · [Español](README.es.md)

> **Status:** implementação inicial. O compilador faz checagem, executa (`paco run`, com cache via Cranelift) e compila para binários nativos via Cranelift e LLVM (`paco build`); a biblioteca padrão ainda é mínima.

## Sumário

- [Sobre](#sobre)
- [Princípios](#princípios)
- [Ecossistema](#ecossistema)
- [Primeiros Passos](#primeiros-passos)
- [Uso](#uso)
- [Estrutura do Repositório](#estrutura-do-repositório)
- [Roteiro](#roteiro)
- [Contribuindo](#contribuindo)
- [Licença](#licença)
- [Contato](#contato)

## Sobre

Serviços, CLIs, jogos, fluxos críticos, compiladores e sistemas de IA compartilham uma mesma base; construir sistemas de IA de ponta a ponta é o critério de desempate quando dois designs conflitam ([RFC 0013](https://github.com/pacolang/rfcs/blob/main/text/0013-ai-systems-north-star.md), [RFC 0028](https://github.com/pacolang/rfcs/blob/main/text/0028-general-purpose-ai-tie-breaker.md)).

Paco está em estágio inicial de design/bootstrap. `AGENTS.md` é a referência rápida da linguagem; `docs/design/spec.md` é a especificação completa (o "o quê"); `docs/implementation/requirements.md` é o roteiro (o "como construir").

## Princípios

1. **Opinativo, mas com liberdade** — um jeito recomendado, com escapes explícitos.
2. **Custo visível** — nenhuma alocação, cópia ou comportamento dinâmico escondido.
3. **Baixo custo mental por padrão** — complexidade só aparece quando você precisa dela.

## Ecossistema

Uma organização, `github.com/pacolang`, um repositório por projeto. Este repositório, `pacolang/paco`, é o núcleo: compilador, runtime e `stdlib`, uma única versão. Cada biblioteca de domínio oficial — `pacolang/tensor`, `pacolang/math`, `pacolang/blas`, e outras conforme forem propostas — recebe seu próprio repositório, sua própria versão semântica, e sua própria declaração de faixa de versão do compilador no seu `paco.mod`; nenhuma delas é distribuída junto com este repositório. Um programa obtém uma com `paco get github.com/pacolang/<nome>@<versão>`, da mesma forma que obtém qualquer outra dependência. `docs/ecosystem.md` tem os critérios de admissão, as regras de dependência e a lista completa atual ([RFC 0030](https://github.com/pacolang/rfcs/blob/main/text/0030-repository-organization-and-stdlib-scope.md)).

`stdlib/numerics.paco`, `stdlib/math.paco` e `stdlib/blas.paco` (`Tensor`, `Matrix`, `DataFrame` e o binding de BLAS) ainda vivem neste repositório hoje — estão marcados para extração para `pacolang/tensor`, `pacolang/math` e `pacolang/blas` respectivamente, numa mudança futura e dependente.

Os programas de exemplo vivem no próprio repositório deles, [`pacolang/examples`](https://github.com/pacolang/examples) — não aqui.

## Primeiros Passos

### Pré-requisitos

- O toolchain do Rust fixado em `rust-toolchain.toml`.
- LLVM 18.1 para o backend otimizador (veja `compiler/paco-codegen-llvm/README.md`). Sem ele, `cargo build --no-default-features -p paco-driver` compila um `paco` cujo `run` e `build` usam só o Cranelift.
- macOS: as ferramentas de linha de comando do Xcode (`xcode-select --install`).
- Windows: as MSVC build tools e o Windows SDK.

### Instalação

Paco compila e roda em Linux (`x86_64`, `aarch64`), macOS (`aarch64`, `x86_64`) e Windows (`x86_64`). No Linux, `paco build` não precisa de nada além dos pré-requisitos acima (`scripts/dist.sh` empacota uma distribuição autocontida).

```bash
cargo build --release
```

## Uso

```bash
paco new hello               # cria um projeto
paco run                     # compila (com cache) + executa
paco build --release         # backend otimizador, binário único
paco test                    # roda funções #[test]
paco fmt --write             # formatador canônico
```

Veja [`pacolang/examples`](https://github.com/pacolang/examples) para programas completos e executáveis.

## Estrutura do Repositório

```
paco/
├── AGENTS.md            # contexto para agentes de IA
├── README.md
├── docs/
│   ├── design/          # a especificação da linguagem
│   ├── implementation/  # requisitos + roteiro
│   └── grammar/         # tokens e gramática EBNF
├── tests/conformance/   # testes: input.paco + saída esperada, rodados em cada backend
├── compiler/            # RUST — o compilador
├── runtime/             # RUST — scheduler, channels, I/O poller (linkado nos binários)
└── stdlib/              # PACO — a biblioteca padrão
    ├── core/            #   prelude: Option, Result, traits, collections, derive
    ├── io.paco, string.paco, sync.paco, collections.paco, dims.paco, autodiff.paco
    └── math.paco, numerics.paco, blas.paco   # marcados para extração, veja docs/ecosystem.md
```

A fronteira Rust ↔ Paco: o que roda **abaixo** da linguagem (compilador, runtime) é Rust; o que vive **dentro** da linguagem (biblioteca padrão) é Paco.

## Roteiro

Veja `docs/implementation/requirements.md` para o plano de construção, e as [issues](https://github.com/pacolang/paco/issues) e [milestones](https://github.com/pacolang/paco/milestones) deste repositório para o acompanhamento do dia a dia.

## Contribuindo

Contribuições são o que fazem da comunidade open-source um lugar incrível para aprender e criar. Qualquer contribuição sua é **muito bem-vinda**.

1. Faça um fork do repositório.
2. Crie sua branch de feature (`git checkout -b feat/minha-feature`).
3. Leia `AGENTS.md` antes de mexer em qualquer arquivo `.paco` ou no código do compilador.
4. Faça commit das suas mudanças e abra um pull request.

Para uma mudança de design de linguagem ou de ecossistema, abra uma RFC em [`pacolang/rfcs`](https://github.com/pacolang/rfcs) primeiro.

## Licença

Distribuído sob a Apache License, Version 2.0. Veja [`LICENSE`](LICENSE) para mais informações. Programas construídos com Paco podem ser lançados sob qualquer licença.

## Contato

Link do projeto: [https://github.com/pacolang/paco](https://github.com/pacolang/paco)
