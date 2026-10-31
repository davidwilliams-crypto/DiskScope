# Search scanned files

Press **Ctrl+F** after starting a scan to focus search. From Overview or Changes, it opens Largest files. Search also works in Explorer and Types. Hover over search for a reminder or press **F1** for examples, including while typing.

Search uses the inventory already in memory. It does not read file contents, follow links, or start another scan. A running or partial scan can only find files it has discovered. Names and paths match case-insensitively. The **Whole scan** checkbox extends the search beyond the current folder; the map keeps its current folder and highlights matching files and ancestors. Separate Filters intersect the search.

| Query | Finds |
|---|---|
| `annual report` | Names containing both words, in any order |
| `"annual report"` | Names containing that exact phrase |
| `report -draft` | Names containing report and excluding draft |
| `name:backup` | Names containing backup |
| `path:Downloads` | Files whose full displayed path contains Downloads |
| `path:"D:\Some Folder"` | Files whose full path contains D:\Some Folder; either slash direction works |
| `ext:zip,7z` | Files with either extension |
| `-ext:zip,7z` | Files with neither extension |
| `ext:"(no extension)"` | Files without an extension |
| `size:>=1GiB` | Files with at least 1,073,741,824 logical bytes |
| `size:>1.5MB size:<=2MiB` | Files over 1,500,000 and at most 2,097,152 logical bytes |
| `path:Downloads ext:zip,7z size:>=1GiB -draft` | Large download archives excluding names containing draft |

All terms are combined with AND. Commas provide alternatives inside `ext:`. Sizes support `=`, `>`, `>=`, `<` and `<=`; omitting the comparison means equality. Bare numbers and B mean bytes. KB, MB, GB, TB, PB and EB use powers of 1000; KiB, MiB, GiB, TiB, PiB and EiB use powers of 1024. Decimals must resolve to whole bytes, and values beyond an unsigned 64-bit byte count are rejected. No rounding or saturation occurs. Logical size does not imply reclaimable space, particularly for sparse files and hardlinks.

Use quotes for literal spaces or a name starting with minus: `"-draft"`. Backslashes stay literal, so Windows paths need no extra escaping. Unknown prefixes remain literal filename text. General wildcard and regular-expression syntax is not supported; `ext:*.zip` is accepted as an extension spelling.

Search accepts at most **1024 characters and 32 terms**. Missing values, invalid sizes and unclosed quotes show a correction message and produce no new matches. Correct the query or select **Clear search and filters**. Invalid search syntax is disclosed in exported requested-view context, including redacted reports, without exposing the search text.

Early result pages retain at most twice their ranking window in candidate IDs, independent of the number of matches. Windows through the first 8192 results use this bounded selection; deeper pages retain the existing full candidate selection fallback. Match highlighting, the inventory itself, and expanded Explorer sorting have their own storage costs. See [validation and measurements](validation-investigation-performance-2026-10-31.md).
