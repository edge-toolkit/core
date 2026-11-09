# Joins scc's per-language counts for the non-test and test partitions into one tab-separated row per language.
# The input is `{"nontest": <report>, "test": <report>}`, each an scc json report. A row is the language, its non-test
# and test code lines and their total, and the rows run from the largest total down.
. as $doc
| (reduce $doc.nontest[] as $l ({}; .[$l.Name] = { nontest: $l.Code, test: 0 }))
| (reduce $doc.test[] as $l (.; .[$l.Name] = { nontest: (.[$l.Name].nontest // 0), test: $l.Code }))
| to_entries
| map({ lang: .key, nontest: .value.nontest, test: .value.test, total: (.value.nontest + .value.test) })
| sort_by(-.total)
| .[]
| [.lang, .nontest, .test, .total]
| @tsv
