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
- Polyphony: N-voice fan-out from a single source and explicit mixing for
  fan-in
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
module <name>[N] : <TypeName> { ... }  # poly: N identical instances
```

`<TypeName>` is resolved by `patches-interpreter` against the module factory
registry. `<name>` becomes the `NodeId` (or the namespace prefix for poly and
template instances — see below).

### Port references

A port is addressed by its module name, port label, and optional index:

```
<name>.<label>        # index 0 implied
<name>.<label>[k]     # explicit index k
<name>.<label>[*]     # all ports with this label (poly contexts only)
```

For modules with a single port per label (the common case), the `[k]` suffix is
omitted. For factory-configured multi-port modules (e.g. `Mixer { channels: 4 }`)
the index selects among the factory-produced ports.

### Connections

```
<port-ref> -> <port-ref>            # unit scale
<port-ref> -> <port-ref> * <number> # explicit scale (any finite f64)
```

The `->` arrow is the only connection operator. Scale is a postfix multiplier on
the destination side; it corresponds to the `scale` field on a graph edge. The
scale may be negative (phase inversion).

### Poly connections

When `[*]` appears on either side of a connection, the expander resolves it
using the known voice count:

```
src[*] -> dst[*]     # zip: connects index 0→0, 1→1, … (counts must match)
src    -> dst[*]     # broadcast: one source to each indexed port on dst
```

Fan-in from poly to a single module requires an explicit mixer:

```
module mix : Mixer { channels: 4 }

voices[*].out -> mix.in[*]   # zip: voice 0 → mix.in[0], etc.
mix.out_l -> out.left
mix.out_r -> out.right
```

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
module voices[4] : voice
```

During expansion (Stage 2 of the pipeline), internal `NodeId`s are namespaced:
`v1/osc`, `v1/env`, etc. Top-level edges referencing `v1.freq` are rewritten to
target `v1/osc.freq`. Templates may be nested.

### Top-level patch block

All module declarations and connections at the root of the file must appear
inside a `patch { ... }` block. Template definitions appear outside it.

```
template voice { ... }

patch {
    module clock : Clock  { bpm: 120.0 }
    module seq   : StepSequencer {
        steps:  [60, 62, 64, 65, 67, 69, 71, 72]
        length: 8
    }
    module alloc        : VoiceAllocator { voices: 4 }
    module voices[4]    : voice
    module mix          : Mixer { channels: 4 }
    module out          : AudioOut

    clock.tick  -> seq.clock
    seq.note    -> alloc.note
    seq.gate    -> alloc.gate

    alloc.freq[*] -> voices[*].freq
    alloc.gate[*] -> voices[*].gate
    alloc.vel[*]  -> voices[*].vel

    voices[*].audio -> mix.in[*]

    mix.out_l -> out.left
    mix.out_r -> out.right
}
```

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
ModuleDecl = "module" Ident PolySpec? ":" Ident InitBlock?
PolySpec   = "[" Nat "]"
InitBlock  = "{" (Ident ":" Value ","?)* "}"

Value      = Table | Array | Scalar
Array      = "[" (Value ","?)* "]"
Table      = "{" (Ident ":" Value ","?)* "}"
Scalar     = Float | Int | Bool | String

Connection = PortRef "->" PortRef Scale?
Scale      = "*" Number
PortRef    = Ident "." Ident Index?
Index      = "[" (Nat | "*") "]"
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
   at the lexical level, or treat all numbers as `f64` and let the factory
   schema coerce? An integer in a `steps` array is naturally a MIDI note
   number (i64), not a float.

## Consequences

**The syntax is intentionally minimal on first pass.** There is no implicit
fan-in, no operator overloading, no conditional logic. These can be added as
sugar once practical use reveals which shortcuts are genuinely common.

**The `->` operator is uniform.** Mono, poly-zip, poly-broadcast, and scaled
connections all use the same operator; the `[*]` index and `* scale` modifiers
distinguish them. This avoids proliferating sigils.

**Templates are a textual abstraction only.** They have no runtime
representation; the expander inlines them completely. A GUI representation of
template instances (showing them as collapsed sub-patches) would need to
preserve template structure alongside the flat graph — this is a concern for a
future GUI layer, not for the current pipeline.
