# G-1…G-4 licensing readiness report

- **Date:** 2026-10-01
- **Scope:** Read-only audit of the current repository and external licensing
  sources for the four licensing gates defined in ADR 0017 §10.4. This report
  makes **no legal conclusion** and implements nothing.
- **Starting points:** ADR 0017 §10 (the licensing research memo),
  `experiments/music-similarity-eval/FINDINGS.md` §3–4, `REPORT.md` §11, and the
  `musicpack-author` README licensing posture.

## How to read this report

Every claim is tagged with the kind of evidence it rests on:

| Tag | Meaning |
| --- | --- |
| **[License text]** | The text of a published license (e.g. CC BY-NC-SA 4.0 legal code). |
| **[Creator statement]** | A statement by the model creators (MTG / Essentia) on their public pages or papers. |
| **[Technical fact]** | A verifiable fact about the artifacts, code, or runtime. |
| **[Legal interpretation]** | A legal reading of the above. **Not legal advice.** |
| **[Unresolved]** | A question the available evidence cannot answer. |

## Threshold finding: the governing license is disputed

Before any gate can be answered, the license that actually governs the
Discogs-EffNet artifacts must be determined. **The licensor's own materials
conflict.** This is a threshold blocker for G-1, G-2, and G-4.

| Source | What it says | Tag |
| --- | --- | --- |
| `essentia.upf.edu/models.html` | "All the models created by the MTG are licensed under **CC BY-NC-SA 4.0** … Check the LICENSE of the models." | [Creator statement] |
| `essentia.upf.edu/licensing_information.html` | "All the models are available under the **CC BY-NC-ND 4.0** license." | [Creator statement] |
| `essentia.upf.edu/models/LICENSE` | Header: "Creative Commons Attribution-NonCommercial-**NoDerivatives** 4.0". But the same file's summary says "You are free to: Share … **Adapt — remix, transform, and build upon the material**" and its legal-details link points to the **by-nc-sa** legal code. | [License text] |
| Model metadata JSON (`discogs_multi_embeddings-effnet-bs64-1.json`) | **No license field at all.** | [Technical fact] |

The repository already records this conflict: the `musicpack-author` README
states "conflicting license notices on MTG's public model pages and an
unconfirmed artifact-level governing license." ADR 0017 §10.1 records the
weights as "CC BY-NC-SA 4.0" (matching `models.html`).

**[Unresolved] Which license governs — CC BY-NC-SA 4.0 or CC BY-NC-ND 4.0?**
The two licenses differ materially: ND permits producing but **not Sharing**
Adapted Material; SA permits Sharing Adapted Material under ShareAlike terms.
The answer changes the analysis for G-1, G-2, and G-4. This must be resolved
by the licensor (MTG) or by qualified legal review; it cannot be resolved from
the available evidence.

---

## G-1 — meaning of "non-commercial" use

**Question (ADR 0017 §10.4):** What does "non-commercial" mean for MusicPack's
own licence and for its users, for both self-hosted personal use and a future
hosted or commercial offering?

### Current evidence

- **[License text]** Both CC BY-NC-ND 4.0 and CC BY-NC-SA 4.0 define
  NonCommercial identically: "not primarily intended for or directed towards
  commercial advantage or monetary compensation." The definition is
  intention-based and does not name self-hosting, personal use, business use,
  or hosted deployment.
- **[Creator statement]** MTG states the models are "for non-commercial use"
  and "also available under a proprietary license upon request." The Essentia
  *library* is AGPL-3.0 "for non-commercial applications," with a commercial
  license available.
- **[Technical fact]** MusicPack's own code is BSD-3-Clause. The model weights
  are operator-supplied, hash-verified before opening, and never bundled,
  downloaded, or redistributed by the repository. The server performs no
  inference; inference is an opt-in, default-off Author feature.
- **[Legal interpretation]** The NC restriction is a *use* restriction. Whether
  a given deployment is "commercial" is fact-specific and turns on the
  operator's intent and context, not on the software's licence. Self-hosted
  personal use by an individual is *plausibly* NC; a hosted or commercial
  offering is *plausibly* outside NC. Neither reading is established by the
  license text.
- **[Legal interpretation]** MusicPack's own BSD-3-Clause code is not affected
  by the model's NC terms: the model is not a dependency of the code but an
  operator-supplied artifact, and no model terms are absorbed into MusicPack's
  source.

### What can already be concluded

- The NC restriction applies to *use* of the model, and its meaning is
  fact-specific. **[Legal interpretation]**
- MusicPack's own code carries no third-party terms. **[Technical fact]**
- A hosted or commercial deployment is squarely inside this gate and is not
  cleared by the available evidence. **[Legal interpretation]**

### Exact unresolved questions

1. Which license governs (SA vs ND)? *(threshold)*
2. Does "self-hosted personal use" by an individual satisfy NC?
3. Does self-hosting *by or for a business* satisfy NC?
4. Does a hosted offering (even non-profit) satisfy NC?
5. Does running the model locally to produce embeddings constitute a "use"
   restricted by NC?

### What requires legal/licensing confirmation

- A written determination from MTG/UPF clarifying the governing license and
  their interpretation of NC for self-hosted and hosted use.
- Qualified legal review of the intended deployment scenarios.

### Recommended next investigation

1. Request written clarification from MTG (contact on
   `essentia.upf.edu/licensing_information.html`) resolving the SA/ND conflict
   and stating their NC interpretation for self-hosting and hosted use.
2. Obtain a qualified legal opinion on the intended MusicPack deployment
   scenarios under the clarified license.

### Status: **INCONCLUSIVE**

The evidence does not support a conclusion for any deployment scenario, and
the threshold SA/ND conflict is unresolved. Not closed.

---

## G-2 — legal status and redistribution of generated embeddings

**Question (ADR 0017 §10.4):** Do generated embeddings inherit the model's
NC/SA terms, and may a `.mpack` carrying them be redistributed?

### Current evidence

- **[License text]** Both licenses define **Adapted Material** as material
  "derived from or based upon the Licensed Material and in which the Licensed
  Material is translated, altered, arranged, transformed, or otherwise
  modified." Embeddings are *outputs* of running the model, not modifications
  of the weights; whether they are Adapted Material is not addressed by the
  license text.
- **[License text]** ND §2(a)(2): "produce and reproduce, but **not Share**,
  Adapted Material for NonCommercial purposes only." SA §2(a)(2): "produce,
  reproduce, **and Share** Adapted Material for NonCommercial purposes only,"
  subject to ShareAlike.
- **[License text]** Both licenses, §2(a)(4) ("Media and formats; technical
  modifications allowed"): making technical modifications necessary to exercise
  the Licensed Rights never produces Adapted Material.
- **[Technical fact]** Embeddings are deterministic functions of (user audio,
  model weights, preprocessing, runtime). They are not derived from the model
  by modification; they are computed outputs.
- **[Creator statement]** No MTG page states whether embeddings/outputs are
  licensed or restricted. The model metadata JSON is silent.
- **[Legal interpretation]** ADR 0017 §10.2 records the correct posture:
  embeddings are *not* assumed to inherit the model's licence in either
  direction. There is no clear legal precedent for ML-derived artifacts.
- **[Legal interpretation]** A third-party project (mStream) states that
  "datasets derived from these weights … inherit NC-SA terms." This is one
  party's interpretation, **not** authoritative, and is itself premised on the
  SA reading that MTG's own licensing page contradicts.

### What can already be concluded

- Embeddings are not the Licensed Material (the weights). **[Technical fact]**
- The repository makes **no** claim that embeddings are unrestricted, and no
  claim that they are licensed. **[Technical fact]** (ADR 0017 §10.2, author
  README.)
- The redistribution question cannot be answered until the SA/ND conflict and
  the Adapted Material question are resolved. **[Unresolved]**

### Exact unresolved questions

1. Which license governs (SA vs ND)? *(threshold)*
2. Are embeddings "Adapted Material" under the governing license?
3. If they are: does a `.mpack` carrying them constitute "Sharing" them?
4. If they are not: are they unrestricted, or does some other term reach them?
5. Does the server-side similarity index (never leaving the machine) avoid the
   redistribution question, or is it a separate "use" question under G-1?

### What requires legal/licensing confirmation

- A written determination from MTG/UPF on the license status of model outputs.
- Qualified legal review of whether embeddings are Adapted Material and whether
  a `.mpack` carrying them is a redistribution requiring permission.

### Recommended next investigation

1. Resolve the SA/ND conflict (shared with G-1).
2. Ask MTG/UPF in writing whether model outputs (embeddings) are licensed,
   restricted, or unrestricted, and whether a container carrying them may be
   redistributed.
3. Obtain a qualified legal opinion on the Adapted Material question for the
   clarified license.

### Status: **INCONCLUSIVE**

The evidence does not support a conclusion, and the question is now more
complex because the SA/ND conflict changes the analysis. Not closed.

---

## G-3 — Discogs dataset provenance and licensing

**Question (ADR 0017 §10.4):** Are the Discogs research dataset terms compatible
with the artefact's NC/SA terms?

### Current evidence

- **[Creator statement]** The model metadata JSON records the dataset as
  "Discogs-4M (unreleased)", citation "In-house dataset", "4M full tracks
  (3.3M used)". The dataset is **not publicly released**.
- **[Creator statement]** The model paper (Alonso-Jiménez, Serra & Bogdanov,
  ISMIR 2022) states: "Discogs publishes monthly dumps of their data under the
  Creative Commons **CC0** license that we use to label 3.3 million tracks from
  our in-house data collection." The poster likewise states Discogs metadata is
  "released under CC0 license."
- **[Technical fact]** The training data is therefore two-layered: (a) Discogs
  editorial *metadata* (stated CC0 by the creators) and (b) an **in-house audio
  collection** whose terms are not publicly documented.
- **[Unresolved]** The audio collection's terms are unknown; the dataset is
  unreleased, so its terms cannot be audited from public sources.
- **[Legal interpretation]** Even if the metadata is CC0, the model was trained
  on audio whose terms are unknown. Whether the model's NC terms are consistent
  with the training data's terms — and whether any training-data restriction
  "flows through" to the model — is a legal question the available evidence
  cannot answer.

### What can already be concluded

- The Discogs *metadata* is stated CC0 by the model creators. **[Creator
  statement]** (Not independently verified against Discogs' own current terms.)
- The *audio* training data is unreleased and its terms are unknown and
  unauditable from public sources. **[Technical fact]**
- The dataset-terms compatibility question cannot be answered without the
  audio collection's terms. **[Unresolved]**

### Exact unresolved questions

1. What are the terms of the in-house audio collection used for training?
2. Do Discogs' *current* terms (the paper cites CC0 monthly dumps) permit the
   use made of them? Has Discogs' position changed since 2022?
3. Does any restriction in the training data (audio or metadata) flow through
   to the model artifact, or is the model's NC term independent?
4. Is the model's NC license itself consistent with training on CC0 metadata?

### What requires legal/licensing confirmation

- The terms of the in-house audio collection (requires disclosure by MTG/UPF).
- Discogs' current terms for the metadata dumps.
- Qualified legal review of training-data-to-model license compatibility.

### Recommended next investigation

1. Ask MTG/UPF in writing for the terms of the in-house audio collection and
   confirmation of the Discogs metadata license (CC0) and its current validity.
2. Verify Discogs' current data-dump terms independently (Discogs terms of
   service / data dump license).
3. Obtain a qualified legal opinion on whether the training data's terms are
   compatible with the model's NC license.

### Status: **INCONCLUSIVE**

The dataset terms are not auditable from public sources; the audio terms are
unknown. Not closed.

---

## G-4 — model artifact licensing, ONNX conversion, runtime, commercial use

**Question (ADR 0017 §10.4):** Does using an ONNX graph through a pure-Rust
runtime satisfy the producing project's terms, without the AGPL-oriented
library?

### Current evidence

- **[Technical fact]** MusicPack uses the **official ONNX artifacts as
  distributed by MTG** (`discogs_multi_embeddings-effnet-bs64-1.onnx`,
  `discogs_release_embeddings-effnet-bs64-1.onnx`, both listed in Essentia's
  model directory). MusicPack performs **no conversion**; the `.onnx` files are
  the licensor's own distribution format.
- **[Technical fact]** The inference runtime is `rten 0.26.0`, documented in ADR
  0017 §10.1 as **MIT OR Apache-2.0**. There is no Essentia, no Python, no
  FFmpeg, and no AGPL code in the inference path.
- **[License text]** Both CC licenses, §2(a)(4): making technical modifications
  necessary to exercise the Licensed Rights never produces Adapted Material.
  Format conversion (e.g. `.pb` → `.onnx`) is the kind of technical
  modification this clause addresses — but here the conversion was performed by
  the licensor, not by MusicPack.
- **[Creator statement]** MTG states the models are available under CC BY-NC-SA
  4.0 (`models.html`) **or** CC BY-NC-ND 4.0 (`licensing_information.html`),
  and "also available under a proprietary license upon request." The LICENSE
  file states the model files are "copyrighted by Universitat Pompeu Fabra
  2019-2021."
- **[Creator statement]** The Essentia *library* is AGPL-3.0 for non-commercial
  use with a commercial license available. MusicPack does not use the Essentia
  library.
- **[Legal interpretation]** rten's MIT/Apache-2.0 license governs the *runtime*
  only. It does **not** resolve the *model*-license question; the model's terms
  apply to the model regardless of which runtime executes it.
- **[Legal interpretation]** Using the official ONNX artifact through rten is a
  technical fact; whether it satisfies the producing project's terms is a legal
  question. The licensor distributes the ONNX artifact publicly, which is
  evidence they intend it to be used, but the NC/SA/ND terms still apply to that
  use.

### What can already be concluded

- MusicPack performs no model conversion; it consumes the licensor's official
  ONNX artifact. **[Technical fact]**
- The runtime's permissive license does not resolve the model-license
  question. **[Legal interpretation]**
- Commercial use is not cleared under either NC license; a proprietary license
  is available upon request. **[Creator statement]**
- The AGPL-oriented Essentia library is not in the inference path. **[Technical
  fact]**

### Exact unresolved questions

1. Which license governs (SA vs ND)? *(threshold)*
2. Does running the model to produce embeddings constitute a "use" restricted
   by NC?
3. Does the ONNX artifact carry the same terms as the `.pb artifact, or
   different ones?
4. What are the terms of the proprietary/commercial license, and is it
   available for MusicPack's use case?
5. Does the producing project's distribution of the ONNX artifact grant
   downstream users (MusicPack operators) the right to use it in the ways
   MusicPack enables?

### What requires legal/licensing confirmation

- A written determination from MTG/UPF on the governing license and the terms
  for ONNX use through a third-party runtime.
- The terms of the proprietary/commercial license.
- Qualified legal review of whether MusicPack's use (local inference, server
  indexing, optional `.mpack` carriage) satisfies the producing project's
  terms.

### Recommended next investigation

1. Resolve the SA/ND conflict (shared with G-1).
2. Ask MTG/UPF in writing: (a) which license governs the ONNX artifacts,
   (b) whether use through a third-party runtime (rten) is permitted, (c) the
   terms of the proprietary/commercial license.
3. Obtain a qualified legal opinion on MusicPack's specific use pattern under
   the clarified license.

### Status: **INCONCLUSIVE**

The evidence does not support a conclusion, and the threshold SA/ND conflict is
unresolved. Not closed.

---

## Summary

| Gate | Status | Blocking dependencies |
| --- | --- | --- |
| **G-1** non-commercial meaning | **INCONCLUSIVE** | SA/ND conflict; NC interpretation is fact-specific; needs legal review + licensor clarification |
| **G-2** embeddings inheritance / redistribution | **INCONCLUSIVE** | SA/ND conflict; Adapted Material question has no precedent; needs licensor clarification + legal review |
| **G-3** dataset terms compatibility | **INCONCLUSIVE** | Audio training data is unreleased/unknown; metadata CC0 is creator-stated only; needs disclosure + legal review |
| **G-4** ONNX / runtime / commercial use | **INCONCLUSIVE** | SA/ND conflict; ONNX use and proprietary-license terms unknown; needs licensor clarification + legal review |

**No gate is closed.** All four remain open and blocking for any *supported*
Discogs-EffNet profile (ADR 0017 §10.4). The single highest-priority next step
is a **written clarification from MTG/UPF** resolving the SA/ND conflict, as
it is a threshold blocker for G-1, G-2, and G-4.

## Authoritative sources (accessed 2026-10-01)

- Essentia models page — https://essentia.upf.edu/models.html (states CC BY-NC-SA 4.0)
- Essentia licensing page — https://essentia.upf.edu/licensing_information.html (states CC BY-NC-ND 4.0)
- Essentia models LICENSE — https://essentia.upf.edu/models/LICENSE (header ND; summary and legal-code link SA)
- Model metadata JSON — https://essentia.upf.edu/models/feature-extractors/discogs-effnet/discogs_multi_embeddings-effnet-bs64-1.json (no license field)
- Model directory listing — https://essentia.upf.edu/models/feature-extractors/discogs-effnet/ (official `.onnx` artifacts)
- CC BY-NC-ND 4.0 legal code — https://creativecommons.org/licenses/by-nc-nd/4.0/legalcode
- CC BY-NC-SA 4.0 legal code — https://creativecommons.org/licenses/by-nc-sa/4.0/legalcode
- Model paper — Alonso-Jiménez, Serra & Bogdanov, "Music Representation Learning Based on Editorial Metadata from Discogs," ISMIR 2022 — https://repositori.upf.edu/handle/10230/54473
