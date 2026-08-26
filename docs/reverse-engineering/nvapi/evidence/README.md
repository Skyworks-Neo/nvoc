# NVAPI generated evidence

These text files are generated inputs for the R610.74 handler-triage reports.
They are kept separate from conclusions so a future audit can regenerate or
diff the evidence without rewriting the reports.

| File | Contents |
|---|---|
| [`query-interface-table-r610-74.txt`](query-interface-table-r610-74.txt) | Full static `nvapi_QueryInterface` dispatch table, including shared-stub flags |
| [`unregistered-handlers-1.txt`](unregistered-handlers-1.txt) | First batch of non-stub IIDs absent from the audited registry |
| [`unregistered-handlers-1-remaining.txt`](unregistered-handlers-1-remaining.txt) | Batch-1 handlers left uncertain after deep analysis |
| [`unregistered-handlers-2.txt`](unregistered-handlers-2.txt) | Second batch of non-stub IIDs absent from the audited registry |

Addresses are virtual addresses from the analyzed binary and are not stable
across driver builds. The proprietary binary and disassembler database are not
part of this repository.
