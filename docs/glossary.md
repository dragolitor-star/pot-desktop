# Glossary System Specification (CTranslate)

This document provides a detailed overview of the Glossary system built during Phase 1 of CTranslate (a fork of Pot).

---

## 1. Glossary Resolution Rules (Priority Order)

When translating text, the system fetches active glossary entries from the SQLite database and applies them. Multiple entries might match the same source term under different conditions. The resolution order is governed by the following strict priority rules (as defined in Directive #4):

1. **Exact Language Match beats Wildcard:**
   - An exact match on both `source_lang` and `target_lang` has higher priority than a wildcard match (`*`).
   - For example, if translating `en` to `tr`, an entry with `en -> tr` will be prioritized over an entry with `* -> tr` or `en -> *` or `* -> *`.

2. **Domain-Scoped Match beats Global Match:**
   - If a translation request provides a specific `scope` (e.g., `tech`), an entry with `scope = 'tech'` has higher priority than a global entry where `scope IS NULL`.
   - Global entries are matched as fallbacks for all scopes.

3. **Most Recently Updated Wins Ties:**
   - If multiple entries match with the same language and scope specificity, the entry with the most recent `updated_at` timestamp (i.e. `datetime(updated_at) DESC`) is chosen.

4. **Inactive Entries are Skipped:**
   - Any entry with `active = 0` (or `false`) is completely ignored and will not participate in the resolution or translation process.

### SQL Implementation
This logic is implemented directly in the SQLite query using `CASE` expressions in the `ORDER BY` clause:
```sql
SELECT id, source_term, target_term, source_lang, target_lang, scope, case_sensitive, active, notes, created_at, updated_at 
FROM glossaries
WHERE active = 1
  AND (source_lang = ?1 OR source_lang = '*')
  AND (target_lang = ?2 OR target_lang = '*')
  AND (?3 IS NULL OR scope IS NULL OR scope = ?3)
ORDER BY
    (CASE WHEN source_lang = ?1 AND target_lang = ?2 THEN 0 ELSE 1 END) ASC,
    (CASE WHEN ?3 IS NOT NULL AND scope = ?3 THEN 0 ELSE 1 END) ASC,
    datetime(updated_at) DESC;
```

---

## 2. Platform / Engine Limitations (Directive #5)

Currently, the glossary system is wired into the **built-in engines** (e.g., Gemini Pro, OpenAI, ChatGLM, Ollama) and classical engines. 

- **Built-in LLM Engines:** Glossary terms are dynamically injected into the system prompt template as a JSON-formatted instruction block.
- **Classical Built-in Engines:** Glossary terms are applied as a post-translation regex replacement over word boundaries.
- **`.potext` Plugins Limitation:**
  > [!WARNING]
  > External `.potext` plugin translation engines are **skipped** in Phase 1 and do not currently support glossary resolution. This is a documented limitation and is marked with `// TODO(phase-X): glossary support for .potext plugins` inside `TargetArea/index.jsx`.

---

## 3. Data Formats

### JSON Import/Export Format Spec
The Glossary system supports importing and exporting glossary data via a standard JSON format. The JSON file must contain a top-level array of glossary entry objects.

#### Example JSON File
```json
[
  {
    "source_term": "hello",
    "target_term": "merhaba kanka",
    "source_lang": "en",
    "target_lang": "tr",
    "scope": "conversational",
    "case_sensitive": false,
    "active": true,
    "notes": "A friendly informal greeting"
  },
  {
    "source_term": "multithreading",
    "target_term": "çoklu iş parçacığı",
    "source_lang": "en",
    "target_lang": "tr",
    "scope": "tech",
    "case_sensitive": true,
    "active": true,
    "notes": "CS term"
  }
]
```

### CSV Format Spec
If imported/exported or managed externally via CSV, the expected headers are:
`source_term,target_term,source_lang,target_lang,scope,case_sensitive,active,notes`

#### Example CSV Row
```csv
source_term,target_term,source_lang,target_lang,scope,case_sensitive,active,notes
hello,merhaba kanka,en,tr,conversational,0,1,A friendly informal greeting
```

---

## 4. LLM Injection Prompt Example

For LLM engines (like Gemini, OpenAI, etc.), active glossary terms are collected and injected into the user prompt or system instructions using the following JSON instruction block format:

```text
[Glossary — use these mappings strictly. Do not translate or modify the target terms. Keep capitalization if case_sensitive is true.]
{
  "hello": "merhaba kanka",
  "multithreading": "çoklu iş parçacığı"
}
```

---

## 5. Post-Translate Word Boundary Logic & Edge Cases

For classical (non-LLM) engines, the glossary is applied to the final translated text. 
We use Unicode-aware word boundaries (`\p{L}\p{N}_`) to prevent replacing substrings within larger words, while correctly handling non-ASCII accented characters (common in languages like Turkish, German, French, etc.).

### Word Boundary Regex Pattern
```javascript
const regexStr = `(?<![\\p{L}\\p{N}_])${escapeRegExp(entry.source_term)}(?![\\p{L}\\p{N}_])`;
const regex = new RegExp(regexStr, entry.case_sensitive ? 'gu' : 'giu');
```

### Turkish Accent Edge Cases
For example, if the glossary contains `şirket -> firma`:
- In the text `"Bu şirket çok büyük."`, `"şirket"` matches exactly because it is surrounded by spaces/punctuation.
- In `"şirketler"`, `"şirket"` will **not** be replaced because `"l"` is a Unicode letter (`\p{L}`), indicating it is part of a larger word.
- Unicode flag `'u'` ensures characters like `'ş'`, `'ı'`, `'ğ'`, `'ö'`, `'ç'`, `'ü'` are correctly treated as letters.
