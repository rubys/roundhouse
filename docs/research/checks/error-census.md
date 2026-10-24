# Error census

How to run and read the errors row in [Foundations](../foundations.md#how-results-are-measured). A raw error count misleads both ways: a change can hide errors behind `untyped`, and removing absorption surfaces every error it hid. The census pairs call sites between a baseline and a candidate and labels each difference.

## Running it

Run one `fixpoint-next` binary twice on the same public app, with `RH_ERRGATE=1 RH_PUBLIC_INPUT=1` both times and the candidate's flags only the second time. With the [setup](../README.md#running-a-check), main against S3:

```sh
PROBE_BASE=1 ./probe discourse RH_ERRGATE=1 RH_PUBLIC_INPUT=1 >/dev/null && mv probe-discourse.stderr main.stderr
./probe discourse RH_ERRGATE=1 RH_PUBLIC_INPUT=1 >/dev/null && mv probe-discourse.stderr s3.stderr
python3 rh/tools/errgate.py main.stderr s3.stderr | tail -n 1
```

The comparator refuses dumps from different builds or inputs. Its last line summarizes the verdicts and each kind's change, raw and corrected (exposed and exposed-pending subtracted).

## Verdicts

An error only the candidate reports is:

- **exposed:** the baseline's receiver was `T | untyped`, or `T` with a pending arm, and no arm of `T` answers the call in the analyzer's method catalog. Only the extra arm kept it silent.
- **exposed-pending:** the baseline's receiver was only `untyped` or pending (besides `nil`), and every arm of the candidate's receiver is one the baseline held, then dropped, in the return slot the receiver comes from.
- **regressed:** a baseline answering-arm label is absent from candidate labels; rendering differences can trigger this without a demonstrated lost runtime possibility.
- **undetermined:** anything else, including copies of one site that pair differently.

An error only the baseline reports is:

- **hidden:** the candidate's receiver has an `untyped` arm that absorbs the call; silenced, not fixed.
- **gained arm:** an arm of the candidate's receiver answers the call, though that arm may not occur at runtime.
- **dead or pending:** the candidate's receiver is pending with no arms: a dead branch or an unfinished value.
- **missing:** the candidate has no census row at that site.

Anything else is undetermined. Rows exist only for calls with an explicit receiver and for arithmetic and comparison operators, so `ivar_unresolved` and `unsupported` changes are always undetermined or missing.

## Reading it

- **Fewer can mean hidden.** On the October 10 condition, eight of Discourse's twelve removed errors are hidden, not fixed ([F27](../facts.md#f27)). F6's ten of thirteen remains the historical result on its older pin.
- **Exposed means absorption used to hide the error, not that the error is real.** The verdict trusts the catalog: it shows neither that the failing arm reaches the call nor that the app lacks the method. In the `RH_DET` trial, source review confirmed none of the new dispatch errors as real ([F14](../facts.md#f14)). Check exposed errors against the source or the runtime oracle before counting them as bugs. The corrected change is accounting, not a count of real regressions.
- **Zero regressed is not soundness.** A receiver that was pending in the baseline has no answering arm to lose.
