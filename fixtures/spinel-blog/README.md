# spinel-blog fixture

Hand-written specimen of `fixtures/real-blog/` lowered into a metaprogramming-free
Ruby subset compatible (over successive iterations) with [Spinel](https://github.com/matz/spinel).

This fixture acts as the **contract** for what a future Roundhouse Phase-1 lowerer
must produce when targeting Ruby. The accompanying tests are the validation oracle:
the same tests pass against this hand-written specimen today, and against
transpiler-generated output once that exists.

## Running

```sh
bundle install
bundle exec rake               # runs tests + linter
bundle exec rake test          # tests only
bundle exec rake lint          # spinel-subset compliance check
```

## Iteration goals

Each iteration narrows the Ruby dialect toward what Spinel can ingest, while
keeping the tests passing:

1. **Iteration 1** — runs on CRuby with idiomatic Ruby; avoids the obvious
   metaprogramming (`instance_variable_get/set`, `define_method`, `send` with
   non-literal symbol, `eval` family).
2. **Iteration 2** — drops threads, multi-encoding, anything Spinel structurally
   rejects.
3. **Iteration 3** — narrows lambda usage to fit Spinel's "no deeply nested
   lambdas with `[]` calls" constraint.
4. **Iteration 4** — actually compiles under Spinel.

`tools/check_spinel_subset.rb` enforces the gradient: each iteration adds rules,
older rules stay.
