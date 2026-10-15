# Ruby lookup-index port

This candidate ports the indexing algorithms from `tobi/roundhouse`, inspected
at `fd0b8211adb6d6938b2069512da99747f75b0267`, onto upstream-compatible
`b91b2efabf4c66ee604ce52994f5a0ffb2d1dd35`. It is not a wholesale copy of
the fork's newer Ruby emitter. The base's semantics, including first-match
ordering and every ordered library reopening, remain authoritative.

## Source-to-port map

All commit links below refer to the public source fork. Paths in the port
column are relative to this repository.

| Source commit / function | Port and adaptation |
| --- | --- |
| [8465feb5](https://github.com/tobi/roundhouse/commit/8465feb58a07566f0a55807d613bc55cd3041bbb): `ConstantReaderIndex`, `constant_readers` | `src/emit/ruby/library.rs`: the original inverted constant-path -> ordered reader-class positions index and indexed consumer. It walks constant initializers and captured class-body calls. It also walks `class_ivar_initializers`, because the upstream fallback already includes them. Qualified and bare nested reads, own-class exclusion, and `BTreeSet` require deduplication are unchanged. The unrelated git-gem work is excluded. |
| [9e67049c](https://github.com/tobi/roundhouse/commit/9e67049c172a6eaf2a366e7c696a5f057116641b): `NameIndex`, `library_classes_named` | Same file: original model/controller membership sets and name -> **all** ordered library positions. `is_app_class`, `owns_a_file`, `file_owner` and `require_path_for_body_const` use them. Current upstream's Data-factory ownership check uses the same ordered lookup without importing the fork's other ownership semantics. The fork's `body_files`/same-source-file merge feature is absent here, so that consumer has no counterpart. |
| [9e67049c](https://github.com/tobi/roundhouse/commit/9e67049c172a6eaf2a366e7c696a5f057116641b): `restore_extras_facades` | Skip its entire library re-emit when the assembled tree contains none of the facade paths. Both `.rb` and `.rbs` paths use this base's facade registry. |
| [46636c34](https://github.com/tobi/roundhouse/commit/46636c34c9f57c9135b690d1b1f848f388970a42): parent lookups | `outer_class_parent`, `lexical_parent`, `require_path_for_parent` use the name index. As in the original, membership sets do **not** replace first-model/controller retrieval with a new map. A first model/controller with no parent still wins; the first non-module library class wins thereafter. Upstream's additional controller participation in lexical resolution is retained. |
| [0665572b](https://github.com/tobi/roundhouse/commit/0665572b2720cbcc27041750b074eab14c759a19): `LookupIndexes`, `emit_with_assertions` | This base's owning entry is `emit_spinel` in `src/emit/ruby.rs`; its guard covers the model, controller and view emits. `emit_library_class_decls` retains its own nested scope, installed **before** its pre-render passes. |
| [0665572b](https://github.com/tobi/roundhouse/commit/0665572b2720cbcc27041750b074eab14c759a19): `mentions_assoc_class_method` | `apply_scope_lowering` passes its already-computed `all_scope_names` set to `src/lower/scope_chain.rs`; the demand query no longer rebuilds that set per body. No dispatch index is introduced. |
| [c69f0ef3](https://github.com/tobi/roundhouse/commit/c69f0ef3b04ee9578bab5f760c0dd03411c75c3c): `ruby_family_files` | The owner is now `project::ruby_family_runtime_files`. Install both original lookup indexes before `spinel_files`, retaining the borrow through all CRuby/JRuby assembly, including overlays, owners, rooting, layout controllers and autoload work. |
| [c69f0ef3](https://github.com/tobi/roundhouse/commit/c69f0ef3b04ee9578bab5f760c0dd03411c75c3c): `ConstResolver::has_source_namespace` | `src/analyze/body/const_resolution/mod.rs`: the original one-time union of source namespaces plus runtime alias targets whose aliases are source-defined classes. Reuse the constructor's existing collected class names; preserve current Rubydex, nested-namespace and source-fingerprint behavior. The separate CLI exit-time `mem::forget` change is deliberately excluded: it is not an index or a lookup dependency. |
| [7cade689](https://github.com/tobi/roundhouse/commit/7cade689feb2d8dcaa1f2157dec5f563e8f8cfb5): `ModelBodyNames`, `share_model_body_names`, `demanded_names` | `src/lower/model_to_library/schema.rs`: original lazy `Rc<OnceCell<Rc<HashSet<Symbol>>>>`, same slice identity/length selection, borrow guard, restoration and scan fallback. Both column and association Dirty synthesis branches use it. The original demand walk is unchanged. Standard upstream collections replace the fork's deterministic-collection aliases. |
| [7cade689](https://github.com/tobi/roundhouse/commit/7cade689feb2d8dcaa1f2157dec5f563e8f8cfb5): lowering and `ModelSynthesis` | Guard `lower_models_inner_with_ruby_values` (the shared bulk-lowering implementation) once per immutable models slice. This base has no `ModelSynthesis` object; its actual survey is the single loop in `unretained_model_contracts`, guarded once around that loop rather than inventing a new synthesis object. Upstream delegate ingest also calls synthesis across models; guard its immutable blocked/available-method planning phase and **drop before model mutation**. |
| [774ee9f9](https://github.com/tobi/roundhouse/commit/774ee9f955055a85913594adbd8695232675eb5e): assembly demand sharing | `ruby_family_runtime_files` shares a demand cache across the complete immutable assembly. Per-lowering nested guards intentionally retain their original independent scope and restore the outer one. This is **not** a claim of one demand build across all lowering passes. This base lacks the fork's `generated_attribute_methods`/`generated_attribute_method_names` consumer; no substitute is invented. |
| [dfe4e068](https://github.com/tobi/roundhouse/commit/dfe4e068c8b197bbf5f4a5a5342ae23080420d71): `first_module_bare_named`, hooked mixins, `resolve_app_module` | Original lazy first-bare-module map serves this base's `hooked_include_position`. It has no prepend-hook variant. `app_extension_calls` is the current module-lookup consumer: it searches the written name and one nested candidate in **collection order**, unlike the fork's newer innermost-first search. Select the minimum eligible position in the two indexed reopening lists to preserve that contract. |
| [08cf065b](https://github.com/tobi/roundhouse/commit/08cf065b4f5ee75cca0a93a2f65c50a28b11bba2): `app_constant_paths` | Additional applicable index found while auditing the final fork. Port its original lazy set of all app names and namespace prefixes into `NameIndex`; `declares_constant_path` uses it under an installed guard. This base already has superclass/mixin shadowing semantics. Retain its original linear fallback outside a guard rather than importing new resolution behavior. |

### Other traced indexes and dependencies

| Source | Status on this base |
| --- | --- |
| [aa5c5c0c](https://github.com/tobi/roundhouse/commit/aa5c5c0cdd6a026fe876e69c454c4602a2ab3a87): `define_method_loops::Names` / `Lineage` | Owning define-method-loop planner is absent. Existing `model_macros` is not that planner; leave it unchanged, rather than replacing it with a new per-model macro index. |
| [a6aa5137](https://github.com/tobi/roundhouse/commit/a6aa5137c0fff12142ddf7c214592cd5e865d0ff): `sti_subclass_validations::Lineage`, descendant `type_names` | Owning STI subclass-validation pass is absent. No algorithm claimed ported and no unrelated ancestry substitute added. |
| [96d39d5b](https://github.com/tobi/roundhouse/commit/96d39d5b018b78f47d102381ad475364e21d0f51): `ValidationsPort` include closure | `ValidationsPort` and its four consumers are absent. The base's existing `active_model_model` is not that feature; leave it unchanged. |
| [0189383c](https://github.com/tobi/roundhouse/commit/0189383cd95182f6bbe6107c9e3218be474aa949): `sti_bases` | Already present upstream: first-model and first-library-parent maps plus the original bounded ancestry walk. Retained, not reimplemented. Regression coverage includes a reopening whose first parent wins and cyclic ancestry termination. |

The final fork also extends `NameIndex` for declaration-source ownership,
carried sources, compact-header source nesting, conditional declarations,
authored requires/load-cycle components and constant-file anchors. Their
owning consumers (`declared_files`, `carried_sources`, `superclass_nesting`,
`conditional_owner`, `require_guard_owners`, `authored_require_sources`,
`load_cycle_components`, `constant_file_anchors`) do not exist on this base.
Those feature-dependent fields are not copied and are not claimed ported.

## Lifetime and ordering contracts

- Both index guards retain `PhantomData<&App>` and restore the previous
  thread-local slot on drop. A different app cannot see an enclosing app's
  index; nested emits and unwinding restore it.
- The body-demand guard retains `PhantomData<&[Model]>`. Its cache is used
  with the immutable slice for the scoped pass, not carried through mutation.
- No `AppMethodIndex` or `LibraryMacroIndex` is introduced. In particular,
  nil method bodies and no-macro models gain no per-method/per-model whole-app
  index construction. Existing reflective-dispatch logic remains unchanged.
- All reopening lists retain source collection order. The lazy module map
  keeps the first eligible module. No map iteration order selects a winner.
- Library Data-factory lowering creates a distinct `App` snapshot on this
  base. Install fresh guards for that finalized snapshot instead of serving
  it from the original app's index; drop them before restoring the outer app.

## Verification

`src/emit/ruby/lookup_tests.rs` instruments **actual builders**, only under
`cfg(test)`, and calls real emit/ingest/lowering/project entry points. It
checks scan equivalence, first matches and reopenings, nested/different-app
and unwound guards, Data-factory contexts, demand laziness, actual delegate
planning and contract-survey reuse, namespace prefixes and bounded ancestry.
The borrow regression compiles the verbatim guard/cache implementations with
small IR stand-ins: each live-mutation rejection must be `E0502` and has a
compiling drop-before-mutation control.

Observed deterministic counts:

- `emit_library_class_decls`: for 100, 200 and 1,000 classes, one reader index
  and one name index; 100/200/1,000 class visits in each builder. Forced
  original reader scans make 10,000/40,000/1,000,000 visits with identical output.
- `emit_lowered_models`: for 8, 16 and 32 models, one demand walk and exactly
  8/16/32 model visits per pass, versus 8/16/32 uncached builds and 64/256/1,024
  model visits. Both paths emit identical output. A slice with no schema
  consumers builds none.
- Delegate ingest and the actual synthesis survey each build one demand set
  across their two-model fixture.
- Tiny-blog whole Ruby/JRuby assembly: three reader/name index scopes and
  three demand builds, not a per-method index. All 336 Ruby files and 338
  JRuby files match forced scan fallbacks byte-for-byte.

Verification passed: 1,237 library tests (one pre-existing ignored test) and
260 tests across the ten selected integration suites, 1,497 passes in total.
The 13 cache-specific tests were rerun after adding the original-versus-index
visit-count assertions. Native Ruby execution is exercised by the hook and
ancestor suites as well as every synthetic comparison tree.

Reproduce the focused checks after the documented fixture setup:

```sh
cargo test --locked --lib \
  --test ingest --test lowered_ruby_emit --test delegate_lowering \
  --test nested_constant_rooting --test data_factory_constants \
  --test hooked_include_order --test ancestor_scoped_include \
  --test assoc_class_method_scope --test sti_subclass_scope --test docs_references
```

`scripts/bench-ruby-lookup-indexes.py` creates independent synthetic apps,
invokes a baseline and candidate CLI in alternating order, and compares every
emitted file by path and SHA-256. It fails on either compiler's nonzero exit;
it does not pass `--allow-unsupported`. Use binaries built with identical
profiles, for example:

```sh
python3 scripts/bench-ruby-lookup-indexes.py \
  --baseline /path/to/base/roundhouse --candidate target/debug/roundhouse \
  --sizes 16 64 128 --rounds 3 --execute-ruby
```

A verified debug-profile run emitted 451, 835 and 1,347 byte-identical files
for 16, 64 and 128 models respectively, across three alternating pairs each.
The optional Ruby execution probe passed in every tree: model construction,
column reading, pending-change demand and nested constant readers. Whole-CLI
baseline/candidate medians were 0.533/0.535 s, 1.103/1.063 s and 1.871/1.729 s.

Whole-CLI elapsed time is recorded separately from output equality. Shared
machine noise and non-index work make it unsuitable as a general speedup
claim. Deterministic build/visit counts are the scaling regression gate.
The full integration suite and native/other-language SDK matrices remain
upstream CI work; a focused green run is not full CI or merge approval.
