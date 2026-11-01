# Canonical PDF layout export

Pages with pattern colors or shading use a separate text-only overlay path:
original drawing streams stay intact, while verified original text-show operators
are made invisible without changing their advances. The original text remains in
the PDF as invisible, selectable content; do not use this as a redaction tool.
The path currently rejects translated text inside Forms, image replacements,
text clipping, inline images, rotated/offset page boxes, and unmatched operators.
When a small label cannot hold its translation, the exporter replaces it with
a numbered marker and puts the full translation in a matching note at least
8pt high. The note is placed only in light, pixel-verified empty space outside
graphic bounds and other translated text. If no safe space or readable marker
exists, the export fails instead of clipping or covering content. Longer blocks
that cannot fit also fail. Numbers restart on each page.
The cover-heading lane restriction applies only when a separate header shares
the row; mixed-size or differently styled cell lines remain distinct blocks.

The layout export edits the source PDF. The reflow and browser-print exports remain
separate. `extract_pdf_pages` keeps its legacy string-array contract; the Document
window now uses `extract_pdf_layout` and retains each block's ID and exact source.

## Mapping, text and typography

`document/pdf_blocks.rs` is the single grouping implementation for extraction and
export. A `PageLayout` owns styled `TextBlock`s, source object indices, available
space, alignment, formula protection, and references to text inside vector Forms.
The exporter checks both ID and source text. Missing, ambiguous, stale, duplicated,
untranslated or overflowing blocks cause an explicit error before publishing a PDF.

Mixed CJK and ASCII runs share their actual baseline and table cell. Wrapped body
lines are merged before assigning IDs. Rectilinear path segments determine table
boundaries; bounding boxes of outlined watermark glyphs are not table borders.
Formulas and pure commands stay in their original objects. Explanatory lines beside
equations remain separate blocks. Short figure/table captions have deterministic
translations for supported language labels. TOC titles are translated separately
from page numbers; output leaders use font metrics and a common right edge.
Large chapter titles with an adjacent bitmap prefix retain their original text
start and expand right. Re-centering on the shorter Chinese title would overlap
the translated `Bölüm X` badge. The eight real-manual chapter headings are checked
against their source bitmap bounds in the completed PDF.

Fitting uses free space bounded by text, images and drawn rules, source font size,
actual glyph bounds, 1.12 line spacing, and 100/95/90 percent horizontal scale. Normal
text stops shrinking at 5.5pt. A vector label originally smaller than that retains
its original lower bound. No production path truncates lines, drops unsupported
characters, or paints white rectangles. Unsupported glyphs and impossible fits fail
with a block ID. Installed bold/italic faces and a math-symbol fallback are embedded
and measured using the same font bytes. Existing regular DejaVu Sans remains bundled.
Cramped Form labels can use an installed condensed face at the same minimum size.
Closely stacked Form label lines share one block. Fullwidth punctuation in Latin
translations is mapped to its equivalent punctuation, without discarding content.

Translated page text objects are detached and destroyed. There is a localized
`ManuallyDrop` workaround for **pdfium-render 0.8.37's double destruction** of an
unowned `PdfPageObject::Text`: both the enum and inner variant implement `Drop`.
The inner destructor must run exactly once. Native fixture export exercises this.
The previous heuristic planner and its regression tests live in
`legacy_pdf_layout.rs`, compiled only for tests.

PDFium 7543's UTF-16 text setter splits supplementary Unicode pairs, including
mathematical bold/italic variables. `unicode_text.rs` transports only affected
runs through an internal ASCII marker, then replaces that marker with real font
CIDs using the embedded ToUnicode map before publication. It verifies an exact
Unicode roundtrip and fails if any scalar is unmappable. This preserves the actual
mathematical glyph and copyable text; variables are not reduced to plain letters.

PDFium's Form child collection is read-only for the pinned binding version.
`form_text.rs` therefore removes selected text-show operators using the existing
`lopdf` dependency after PDFium writes the temporary file. It first matches the
complete ordered Unicode text inventory. Vector drawing operators remain intact.
Unsupported encodings or text runs dependent on another run's advance fail safely.
Form child labels use their composed page transform. This includes the manual's
vector diagram on page 77. Nested raster images are also visited by the image path.

## Translation and cache validation

Gemini batches use system instructions, JSON `{id,text}` input, and schema-constrained
`{id,translation}` output. Duplicate, non-integer and unknown IDs never shift another
item. Complete objects can be salvaged from a truncated response without parsing
quoted braces as objects. Missing/ambiguous/residual-CJK items get one isolated repair
attempt before publishing/cache writes. Requests, including repairs, are RPM paced;
429 stops the run. Gemini 3 uses the documented thinking level for the selected model.

The shared cache helper rejects residual CJK for non-CJK targets, including old hits.
PDF text keys include a canonical-pipeline revision and glossary fingerprint so old
marker-drift results cannot poison a new run. Glossary scope was already implemented
as ranking, not exclusion, and its existing resolution tests remain in place.

## Images

Choose **Gemini Vision (in place)** in the image-method selector. New settings default
to this method; an existing saved external-service preference is retained. The old
whole-image OCR overlay option is retired because it destroys diagrams. Its request
field is retained for compatibility and rejected with an actionable error.

`image_translate.rs` extracts all sizes, combines adjacent Word chapter mini-images,
and deduplicates by SHA-256 of pixels, dimensions, source/target language, provider,
model, glossary and algorithm version. Up to four unique images are recognized in
one ID-checked multimodal batch, within a payload budget. Calls go directly to the
configured provider with header authentication; there is no mandatory service backend.

Image decoding includes the PDF soft mask (`SMask`): Word's chapter/step glyphs may
exist only in that mask over a solid RGB bitmap. A local pixel-coordinate render
applies masks/filters and restores the exact source matrix, so rotation/shear are
not baked into pixels twice. Cache algorithm `v5-local-ocr` invalidates earlier
raw-only and model-coordinate results. A native manual fixture checks both real glyph
masks and matrix restoration.
Transient 408/5xx responses receive bounded, paced retries; 429 still stops the run.
Model-provided image boxes are approximate and are never used directly for erasure.
`image_ocr.rs` requests local measured word bounds from the Document window's existing
Tesseract worker. Matching complete source labels to contiguous OCR words resolves
the actual location; repeated labels cannot claim the same measured glyphs. Numeric,
formula and logo regions whose text is unchanged remain in their original pixels.
Unmatched labels, final overlapping boxes or impossible fits fail before publishing.
An invalid untranslated provider item receives one isolated semantic repair.

The local OCR broker uses unique job IDs and export run IDs, responds to cancellation,
and cleans up stale replies. The worker is terminated when the export ends. Its
JavaScript/WASM assets already exist in the project; language weights use the existing
recognition provider's download host. Document pixels stay in the local worker.
No Node installation is needed by the application. The opt-in native verification
uses `.scripts/run_layout_image_ocr.cjs` as its local counterpart, selected explicitly
by `CTRANSLATE_LOCAL_OCR_SCRIPT`.

Before image edits, `image_resources.rs` makes each `Do` occurrence's image and
soft-mask resources private, including images inside reused Forms. PDFium otherwise
shares its internal image across pages: changing the chapter-5 prefix can corrupt
chapters 6–8. This preprocessing keeps operator order, geometry, clipping and
graphics-state operators intact. A real-manual native test proves that replacing
one chapter cannot change another chapter or another page.

Detected regions carry normalized glyph boxes, source/translated text and color
hints. Connected glyph components refine the boxes; source pixels determine actual
foreground color and include antialiased edges. Perimeter sampling and boundary
interpolation restore flat/smooth backgrounds. Component masks protect nearby frames
and icons. A translation can use additional verified empty pixels, bounded by other
labels and graphics; its source erasure remains limited to the original glyph mask.
Connected small text rows inside multiline labels are not classified as rules.
Hollow-frame rejection checks the four actual edges of the component.
Transparent badges restore background alpha. PDFium
renders fitted replacement glyphs onto an alpha layer. The existing bitmap is updated
with its exact matrix. Multi-part Word prefixes are reconstituted in their original
union rectangle so letters can cross the old inter-image gaps. Warning/info icons
outside the detected text boxes remain unchanged.

Cache migration **v2** adds `image_translation_cache` with PNG bytes, region metadata,
language/model fields and LRU timestamps. Both successful translations and textless
images are reusable. Cache clearing includes text and image rows in one transaction.
`sha2`, `lopdf` and `regex` were already compiled transitively; direct imports add no new crate
to the compiled dependency graph. No OCR/inpainting crate was added.

This is a deterministic mask-based repair, not a guarantee of perfect reconstruction
of arbitrary photographs. OCR omissions, text crossing graphics, complex backgrounds,
unsupported shaping/fonts and unusual Form encodings still need visual verification.
The pipeline must not be described as 100% indistinguishable without that verification.

## Verification

Run `pnpm test`, `pnpm exec vite build`, and `cargo test --manifest-path src-tauri/Cargo.toml`.
The original Rust tests are retained, including the legacy geometry suite.

Nine opt-in ignored native tests use `CTRANSLATE_PDFIUM`, `CTRANSLATE_TEST_PDF`,
`CTRANSLATE_AUDIT_DIR` and, for rendering, `CTRANSLATE_LAYOUT_REQUEST`:

- `audit_manual_canonical_blocks`: all pages, deterministic IDs, unique object owners.
- `render_manual_cached_diagnostic`: original PDF plus existing local translations;
  writes an explicitly diagnostic PDF and fitting report, not a verified translation.
- `native_image_text_preserves_background_and_alpha`: actual PDFium text rasterization,
  outside-region pixel preservation and a cache-only image replacement with a rotated/
  sheared matrix; no network.
- `native_masked_chapter_images_retain_text`: source soft-mask decoding and exact
  matrix restoration on real Word badges.
- `native_image_replacement_cannot_change_another_chapter`: private image resources
  isolate another badge on the page and the next page.
- `render_cached_image_regions`: render one local OCR fixture without a provider call;
  successful fitting still requires visual review.
- `render_manual_live_translation`: explicitly authorized provider verification,
  described below.
- `native_supplementary_math_roundtrip`: real embedded math glyphs and exact
  supplementary Unicode extraction after saving/reopening.
- `audit_local_image_geometry`: measures and fits all current image fixtures
  offline. `CTRANSLATE_AUDIT_CACHED=1` also rechecks existing PNG fixtures;
  a successful test still requires reviewing its per-image report and previews.

`.scripts/verify_pdf_pipeline.py --cached` prepares the diagnostic request without
network access. `.scripts/render_pdf_checks.py` compares representative raster pages
and verifies graphic counts. All private/source-derived intermediates stay under
ignored `tmp/pdfs/`.

Final verification of the supplied manual passed: 137 pages and 1,832 stable
canonical blocks, with 1,495 fitted translations and zero fitting/collision errors.
All 1,484 source Han text blocks were replaced, and every fitted line was confirmed
in actual PDF text extraction. No Han, NULL, replacement glyph or Unicode transport
marker was found in extracted page text. Page geometry, vector-path and image counts
matched on every page. All 270 approved image keys were used (606 cache hits and no
new provider calls during assembly). Nine final contact sheets and critical full-size
page/crop views were reviewed, including TOC, equations, chapter headings and appendix
tables. The blurry raster-print exceptions described below remain intentional.
Normal suites passed 195 Rust tests (nine opt-in native tests excluded) and 53
JavaScript tests; the Vite build passed. Four native regressions covering image
resource isolation, alpha/matrices, Word soft masks and supplementary math also
passed, as did the full canonical and native text audits.
The final chapter-placement regression also passed. Only the eight title alignments
changed in the canonical layout; their native-rendered pages were assembled into
the full export while preserving page identities and destinations. The other 129
pages were pixel-identical after assembly. Final checks were rerun on the assembled
137-page artifact before copying it into `output/pdf/`.

The full image review covers 270 unique bitmaps. All 260 selected native renders
passed; seven difficult source-measured repairs and three photo treatments were
reviewed separately. Review included 45 source/result contact sheets, 12 sheets
covering 70 changed results, and full-size critical crops. Source and result hashes
are recorded before copying a result into the final v6 fixture cache. A successful
fit alone is not evidence that all source text was erased. Pixel/OCR flags were
classified explicitly: UI icons and incidental graphics, clipped/blurred background
labels, and the user's requested photo-print exceptions. Those source pixels remain
unchanged; the result must not be described as zero Chinese in every raster image.

The image cache namespace is `image-glyph-inpaint-v6-measured-rows`. Local OCR also
runs when Vision returns no regions, and reliable omitted Han words are recovered
from locally measured crops. OCR confidence is retained through the frontend worker.
Post-render checking rejects reliable source words left pixel-for-pixel unchanged.
Image validation also preserves the multiplicity of literal numbers, error codes,
paths, filenames and technical identifiers. Screenshot source measurements, not
model-proposed coordinates, determine glyph masks and fitting bounds.

The native text review checked command identifiers and filenames in every translated
block and reviewed numeric/fragment outliers. It corrected mispaired continuation
fragments on pages 66, 69 and 91 before final export. The Rust batch worker, Gemini
JavaScript path, cache reads/writes and PDF planner now reject missing command
identifiers or filenames. Invalid batch peers receive one isolated repair; valid
items retain their original IDs. PDF-split identifiers such as `alg_b ias_left`
can be restored as `alg_bias_left`; localized units remain valid.

The image painter separates foreground color families so a red annotation touching
black glyphs cannot become text. Hollow/clipped frames and long rules are excluded
from the mask. Erasure samples clean pixels from the same screenshot row, including
its alpha, instead of borrowing a neighboring dialog's fill or anti-aliased glyph
edge. Measured glyph rows bound the font size, single-line labels cannot grow
vertically, and cramped labels can use the installed Arial Narrow face. Every
selected face is measured and embedded consistently. Missing reliable source ink
is an error rather than permission to paint on an empty rectangle.

On 2026-10-23 the user requested preserving blurry photo print unchanged. Private
source-legibility manifests record this exception; no company name or phone number
is guessed. The readable 工装1 label is translated as Fikstür 1 with a measured
perspective transform and a checked glyph-only pixel mask. The readable main
optoelectronic slogan is translated; blurry company print stays in the source photo.

The user explicitly authorized sending this manual's text and embedded images to
Google Gemini using the saved API key on 2026-10-23. All native text translations
have been refreshed successfully. This authorization is specific to that manual and provider;
it is not blanket permission to transmit other documents.

`render_manual_live_translation` is an opt-in native verification test and
requires `CTRANSLATE_ALLOW_GEMINI=1`. It reads the selected saved key in memory,
preflights every selected text block, then exercises the actual image and text
renderers. Resumable PNG/region fixtures stay in ignored `tmp/pdfs/`; no key is
written there. The normal test suite never calls a provider. Final assembly uses
`CTRANSLATE_CACHE_ONLY=1`, which fails on a missing approved image instead of making
a network request. Final extraction, geometry and visual review must pass before
a PDF is delivered.
