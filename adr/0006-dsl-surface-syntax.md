# ADR 0006 — DSL surface syntax

## Status

Under review

## Context

ADR 0005 defines the compilation pipeline for the patch DSL and the crate
structure that implements it. This ADR defines the *surface syntax* — the
language a patch author writes. It is a companion document to ADR 0005 and
should be read alongside it.

Requirements the syntax must satisfy:

- Module declarations with scalar, array, and table initialisation values
- Patch cable connections with optional scale factor
- Template definitions: named sub-patches with declared signal ports,
  instantiable as if they were primitive module types
- Polyphony: poly cables carrying N channels simultaneously, declared at
  connection time (see ADR 0015); no module duplication syntax
- Port indexing for factory-configured multi-port modules (see ADR 0005)

## Decision

### Lexical conventions

```
# Comment to end of line
Identifiers:  [a-zA-Z_][a-zA-Z0-9_-]*
Numbers:      integer or floating-point literals  (42, 440.0, -0.5)
Strings:      double-quoted  "hello"
Booleans:     true | false
```

### Init-param values

Module declarations carry a `{ key: value }` block of initialisation
parameters, evaluated once at graph-build time by the module's factory.

```
Value  = Scalar | Array | Table
Scalar = Number | String | Bool
Array  = "[" (Value ","?)* "]"
Table  = "{" (Ident ":" Value ","?)* "}"
```

Examples:

```
{ frequency: 440.0 }
{ steps: [60, 62, 64, 65, 67, 69, 71, 72], length: 8 }
{ pattern: [
    { note: 36, velocity: 1.0, gate: 0.5 },
    { note: 38, velocity: 0.7, gate: 0.5 },
] }
```

The `{ ... }` block is optional; omitting it is equivalent to an empty map.

### Module declarations

```
module <name> : <TypeName> { <params> }
module <name> : <TypeName>              # params omitted — empty map
```

`<TypeName>` is resolved by `patches-interpreter` against the module factory
registry. `<name>` becomes the `NodeId` (or the namespace prefix for template
instances — see below). There is no `[N]` poly-duplication syntax; polyphony
is expressed as a property of connections, not module declarations.

### Port references

A port is addressed by its module name, port label, and optional index:

```
<name>.<label>        # index 0 implied
<name>.<label>[k]     # explicit index k
```

For modules with a single port per label (the common case), the `[k]` suffix is
omitted. For factory-configured multi-port modules (e.g. `Mixer { channels: 4 }`)
the index selects among the factory-produced ports. There is no `[*]` wildcard;
that was part of the removed poly-duplication syntax.

### Connections

```
<port-ref> -> <port-ref>                   # mono, unit scale
<port-ref> -> <port-ref> * <number>        # mono, explicit scale (any finite f32)
<port-ref> -> <port-ref> poly <N>          # poly cable, N channels, unit scale
<port-ref> -> <port-ref> poly <N> * <num>  # poly cable, N channels, scaled
```

The `->` arrow is the only connection operator. Scale is a postfix multiplier on
the destination side; it corresponds to the `scale` field on a graph edge. The
scale may be negative (phase inversion).

A `poly N` annotation creates a `CableValue::Poly` buffer slot carrying up to N
channels (N ≤ 16). The graph validator (ADR 0015) rejects a poly connection if
either port's `PortDescriptor` declares `CableKind::Mono`. Omitting `poly` creates
a `CableValue::Mono` connection; the validator rejects it if either port declares
`CableKind::Poly`.

### Template definitions

A template is a named sub-patch with declared input and output signal ports.

```
template <name> {
    in:  <port>, <port>, ...
    out: <port>, <port>, ...

    module ...
    module ...

    <connection>
    ...
    <connection>
}
```

The `in:` and `out:` lines declare the template's external signal interface.
Inside the body, `in`-ports are used as sources and `out`-ports as
destinations:

```
template voice {
    in:  freq, gate, vel
    out: audio

    module osc  : SineOsc
    module env  : ADSR       { attack: 0.01, decay: 0.1, sustain: 0.7, release: 0.3 }
    module filt : LadderFilter { cutoff: 0.5, resonance: 0.2 }
    module vca  : Vca

    freq    -> osc.freq
    gate    -> env.gate
    vel     -> env.vel
    osc.out -> filt.in
    env.cv  -> filt.cutoff * 0.4
    filt.out -> vca.in
    env.cv  -> vca.cv
    vca.out -> audio
}
```

A template is instantiated like a primitive module type:

```
module v1 : voice { ... }    # init params forwarded to internal modules (TBD)
```

During expansion (Stage 2 of the pipeline), internal `NodeId`s are namespaced:
`v1/osc`, `v1/env`, etc. Top-level edges referencing `v1.freq` are rewritten to
target `v1/osc.freq`. Templates may be nested.

### Top-level patch block

All module declarations and connections at the root of the file must appear
inside a `patch { ... }` block. Template definitions appear outside it.

```
patch {
    module clock  : Clock  { bpm: 120.0 }
    module seq    : StepSequencer {
        steps:  [60, 62, 64, 65, 67, 69, 71, 72]
        length: 8
    }
    module alloc  : VoiceAllocator { voices: 16 }
    module osc    : PolyOsc
    module env    : PolyADSR  { attack: 0.01, decay: 0.1, sustain: 0.7, release: 0.3 }
    module filt   : PolyLadder { cutoff: 0.5, resonance: 0.2 }
    module vca    : PolyVca
    module mix    : PolyMix
    module out    : AudioOut

    clock.tick    -> seq.clock
    seq.note      -> alloc.note
    seq.gate      -> alloc.gate

    alloc.freq    -> osc.voct   poly 16
    alloc.gate    -> env.gate   poly 16
    alloc.vel     -> env.vel    poly 16
    osc.out       -> filt.in    poly 16
    env.cv        -> filt.cutoff poly 16 * 0.4
    filt.out      -> vca.in     poly 16
    env.cv        -> vca.cv     poly 16
    vca.out       -> mix.in     poly 16

    mix.out_l     -> out.left
    mix.out_r     -> out.right
}
```

Poly-capable modules (`PolyOsc`, `PolyADSR`, etc.) declare `CableKind::Poly`
on the relevant ports in their `PortDescriptor`. The graph validator confirms
cable types match port declarations before the plan is activated. Mono signals
(e.g. `clock.tick -> seq.clock`) remain `CableValue::Mono` and are validated
against `CableKind::Mono` ports.

### Grammar sketch (PEG notation)

```
File       = Template* Patch
Patch      = "patch" "{" Statement* "}"
Template   = "template" Ident "{" PortDecls Statement* "}"
PortDecls  = InDecl OutDecl
InDecl     = "in:" CommaIdents
OutDecl    = "out:" CommaIdents
CommaIdents = Ident ("," Ident)* ","?

Statement  = ModuleDecl | Connection
ModuleDecl = "module" Ident ":" Ident InitBlock?
InitBlock  = "{" (Ident ":" Value ","?)* "}"

Value      = Table | Array | Scalar
Array      = "[" (Value ","?)* "]"
Table      = "{" (Ident ":" Value ","?)* "}"
Scalar     = Float | Int | Bool | String

Connection = PortRef "->" PortRef PolySpec? Scale?
PolySpec   = "poly" Nat
Scale      = "*" Number
PortRef    = Ident "." Ident Index?
Index      = "[" Nat "]"
```

## Open questions

The following points are not yet settled and are the reason this ADR remains
under review:

1. **Init params forwarded to template internals.** When a template is
   instantiated with a `{ key: value }` block, how are the values routed to
   internal modules? Options: a flat namespace forwarded to all internal
   modules (risky — name collisions), a scoped namespace per internal module
   (`osc.frequency: 440.0`), or template-level param declarations that
   explicitly bind to internal module params. The right answer affects the
   template syntax.

2. **Multiple patches in one file.** Is a single `patch { }` block always
   sufficient, or do we want named patches that can be selected at runtime?

3. **Ordering constraints.** The grammar above allows interleaved module
   declarations and connections. The expander requires all modules to be
   declared before connections are resolved. Should the grammar enforce
   declaration-before-use, or should the expander collect all declarations
   first in a two-pass approach?

4. **Numeric literals.** Should the grammar distinguish integers from floats
   at the lexical level, or treat all numbers as `f32` and let the factory
   schema coerce? An integer in a `steps` array is naturally a MIDI note
   number (i64), not a float.

## Consequences

**The syntax is intentionally minimal on first pass.** There is no implicit
fan-in, no operator overloading, no conditional logic. These can be added as
sugar once practical use reveals which shortcuts are genuinely common.

**The `->` operator is uniform.** Mono, poly, and scaled connections all use
the same operator; the `poly N` annotation and `* scale` modifier distinguish
them. This avoids proliferating sigils.

**Templates are a textual abstraction only.** They have no runtime
representation; the expander inlines them completely. A GUI representation of
template instances (showing them as collapsed sub-patches) would need to
preserve template structure alongside the flat graph — this is a concern for a
future GUI layer, not for the current pipeline.
