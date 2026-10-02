# Licensing clarification package — MusicPack use of Discogs-EffNet

- **Date:** 2026-10-02
- **Status:** Research/documentation only. **This is not a legal opinion.** It
  formulates questions and records evidence; it does not answer the licensing
  questions or recommend production changes.
- **Purpose:** To obtain authoritative clarification of the licensing questions
  that block gates G-1 through G-4 (ADR 0017 §10.4), so those gates can be
  resolved on the basis of evidence rather than assumption.
- **Authoritative starting points (MusicPack's own established record):**
  - `docs/licensing-g1-g4-readiness.md` (read-only audit, 2026-10-01)
  - ADR 0017 §10 (licensing research memo) and §10.5 (G-7 disposition)
  - ADR 0016 (licensing context), `experiments/music-similarity-eval/FINDINGS.md`
    §3–4, `REPORT.md` §11, and the `musicpack-author` README licensing posture.

## How to read the evidence

Every claim is tagged with the kind of evidence it rests on:

| Tag | Meaning |
| --- | --- |
| **[License text]** | The text of a published license (e.g. the CC BY-NC-SA 4.0 legal code). |
| **[Creator statement]** | A statement by the model creators (MTG / Essentia) on their official pages, papers, or model metadata. |
| **[Technical fact]** | A verifiable fact about the artifacts, code, files, or runtime. |
| **[Legal interpretation]** | A legal reading of the above. **Not legal advice.** |
| **[Unresolved]** | A question the available evidence cannot answer. |

Interpretations are never stated as facts. Where the evidence is insufficient,
the gap is recorded as **[Unresolved]**.

---

## 1. Purpose

MusicPack is a self-hosted, open-source (BSD-3-Clause) music player and library
manager. It optionally integrates the Discogs-EffNet music-embedding model to
provide a "similar tracks" capability. The integration is deliberately
conservative: the model weights are **operator-supplied**, hash-verified before
opening, and never bundled, downloaded, or redistributed by MusicPack.

The integration is currently blocked by four licensing gates (G-1…G-4, ADR 0017
§10.4), all of which are **INCONCLUSIVE**. A threshold problem makes the blocker
worse: **the licensor's own materials identify two different licenses** (CC
BY-NC-SA 4.0 and CC BY-NC-ND 4.0), and the model metadata states none.

The purpose of this package is to assemble, in one place, the exact questions
that need authoritative clarification and the evidence already available, so
that a written request can be sent to the model creators (MTG/UPF) and, in
parallel, so that qualified legal review can be scoped. This document does not
answer the questions, does not resolve the gates, and does not recommend
production changes.

---

## 2. Current evidence

### 2.1 The governing license is disputed (threshold finding)

| # | Source (exact) | What it states | Tag |
| --- | --- | --- | --- |
| 1 | `essentia.upf.edu/models.html` — "Essentia models" page | "All the models created by the MTG are licensed under **CC BY-NC-SA 4.0** … and are also available under a proprietary license upon request. Check the LICENSE of the models." | [Creator statement] |
| 2 | `essentia.upf.edu/licensing_information.html` — "Licensing Essentia models" | "All the models are available under the **CC BY-NC-ND 4.0** license for non-commercial use, and are also available under a proprietary license upon request." | [Creator statement] |
| 3 | `essentia.upf.edu/models/LICENSE` — the models' LICENSE file | Header: "Creative Commons Attribution-NonCommercial-**NoDerivatives** 4.0". The same file's human-readable summary says "You are free to: Share — copy and redistribute the material … **Adapt — remix, transform, and build upon the material**", and its "All legal details" link points to the **by-nc-sa** legal code. | [License text] |
| 4 | Model metadata JSON for both variants (see §3) | **No license field.** | [Technical fact] |

**What MusicPack can establish:** the licensor's official materials are
internally inconsistent. `models.html` says SA; `licensing_information.html`
says ND; the `LICENSE` file names ND in its header but describes SA-like
freedoms and links to the SA legal code; the model metadata is silent.

**What remains uncertain:** which license (if either) actually governs the
artifacts MusicPack uses. This is a threshold question: the SA and ND licenses
differ materially (see §2.2), so the answer changes the analysis for G-1, G-2,
and G-4. **[Unresolved]**

### 2.2 The two candidate licenses differ

Both candidate licenses are Creative Commons 4.0 International "NonCommercial"
variants. Relevant differences:

| Provision | CC BY-NC-SA 4.0 | CC BY-NC-ND 4.0 |
| --- | --- | --- |
| NonCommercial definition | "not primarily intended for or directed towards commercial advantage or monetary compensation" | identical |
| Adapted Material definition | "material … derived from or based upon the Licensed Material and in which the Licensed Material is translated, altered, arranged, transformed, or otherwise modified" | identical |
| §2(a)(2) — Adapted Material | "produce, reproduce, **and Share** Adapted Material for NonCommercial purposes only" | "produce and reproduce, but **not Share**, Adapted Material for NonCommercial purposes only" |
| §2(a)(4) — technical modifications | making technical modifications necessary to exercise the Licensed Rights never produces Adapted Material | identical |
| ShareAlike | Adapted Material must be shared under the same license | no ShareAlike (but no Sharing of Adapted Material at all) |

**[License text]** The key difference for MusicPack: under ND, Adapted Material
may not be Shared at all; under SA, it may be Shared for NonCommercial purposes
under ShareAlike terms. Whether embeddings are "Adapted Material" is itself
unresolved (see G-2).

### 2.3 MusicPack's own posture (technical facts)

- **[Technical fact]** MusicPack's own source is BSD-3-Clause. The model is
  not a dependency of the code; it is an operator-supplied artifact.
- **[Technical fact]** Model weights are never committed, bundled, downloaded, or
  redistributed by the repository. The operator supplies the exact file;
  MusicPack verifies its SHA-256 before opening it.
- **[Technical fact]** The server performs no inference. Inference is an
  opt-in, default-off Author feature behind the `discogs-effnet` cargo feature.
- **[Technical fact]** The inference runtime is `rten 0.26.0` (MIT OR
  Apache-2.0 per ADR 0017 §10.1). There is no Essentia, Python, FFmpeg, or
  AGPL code in the inference path.
- **[Technical fact]** Generated embeddings are stored as `f32le`, optionally
  carried in a `.mpack` similarity document, and indexed in a server-local
  similarity index. Raw vectors are never exposed via the API.

### 2.4 What the model metadata establishes

Both model metadata JSON files (see §3) record:

- Model family `EffnetDiscogs`; version `1`; release date `2022-06-15`.
- Framework: TensorFlow `2.8.0`; model types `frozen_model`, `onnx`.
- Author: Pablo Alonso (pablo.alonso@upf.edu).
- Dataset: "Discogs-4M (unreleased)", citation "In-house dataset".
- Inference: sample rate 16000 Hz, algorithm `TensorflowPredictEffnetDiscogs`.
- **No license field.**

**[Creator statement]** The metadata is the creators' own description of the
artifact. It establishes the artifact's identity and training-data reference
but not its license.

---

## 3. Exact artifact identity

MusicPack uses two exact artifacts, recorded in code
(`crates/musicpack-author/src/similarity_effnet.rs`):

| Property | multi | release |
| --- | --- | --- |
| File | `discogs_multi_embeddings-effnet-bs64-1.onnx` | `discogs_release_embeddings-effnet-bs64-1.onnx` |
| Size (bytes) | 15,998,047 | 18,621,961 |
| SHA-256 | `65cfde30655a939de420e5c09a49a43648336fdbbf79eb3af6d3b40176339d8e` | `fb49bd4e906624bbc5ee09e648e88d53b28c23870a8a6bed145624fd062530e7` |
| Embedding output | 1280-D (`PartitionedCall:1`) | 512-D (`PartitionedCall:1`) |
| Profile id | `musicpack-similarity-discogs-effnet-multi-v1` | `musicpack-similarity-discogs-effnet-release-v1` |

### 3.1 Provenance of these exact artifacts

- **[Technical fact]** Both `.onnx` files are listed in Essentia's official
  model directory (`essentia.upf.edu/models/feature-extractors/discogs-effnet/`)
  with file sizes that match MusicPack's records exactly (15,998,047 and
  18,621,961 bytes). This is strong evidence that MusicPack's artifacts are the
  **creator-distributed ONNX files**, not MusicPack conversions.
- **[Technical fact]** The corresponding `.pb` (TensorFlow frozen_model) files
  are also present in the same directory, confirming the ONNX files are a
  creator-provided distribution format, not a MusicPack-produced conversion.
- **[Creator statement]** The model metadata JSON for each variant records
  `model_types: ["frozen_model", "onnx"]`, confirming the creators distribute
  both formats.

### 3.2 What can and cannot be established

| Question | Answer | Basis |
| --- | --- | --- |
| Model version/revision? | `EffnetDiscogs` v1, release date 2022-06-15 | [Creator statement] (metadata JSON) |
| Artifact type? | ONNX graph (creator-distributed); TensorFlow `.pb` also provided | [Technical fact] + [Creator statement] |
| Corresponding license? | **Not established.** Metadata has no license field; official pages conflict (§2.1) | [Unresolved] |
| Are the ONNX files creator-distributed or MusicPack conversions? | **Creator-distributed.** File sizes match the official directory listing; MusicPack performs no conversion | [Technical fact] |
| Do the two variants differ in training data? | Metadata records "3.3M used" (multi) vs "1.7M used" (release) of "4M full tracks" | [Creator statement] (metadata JSON) |

**Exact artifact provenance:** the artifacts' identity (file, size, SHA-256,
version, framework) is established and tied to the creators' official
distribution. The **license** corresponding to these exact artifacts is **not**
established by the available evidence. **[Unresolved]**

---

## 4. G-1 — permitted use

**Gate question (ADR 0017 §10.4):** What does "non-commercial" mean for
MusicPack's own licence and for its users, for both self-hosted personal use
and a future hosted or commercial offering?

### 4.1 Evidence

- **[License text]** Both candidate licenses define NonCommercial identically:
  "not primarily intended for or directed towards commercial advantage or
  monetary compensation." The definition is intention-based and does not name
  self-hosting, personal use, business use, or hosted deployment.
- **[Creator statement]** MTG states the models are "for non-commercial use"
  and "also available under a proprietary license upon request." The Essentia
  *library* is AGPL-3.0 "for non-commercial applications," with a commercial
  license available.
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

### 4.2 Questions requiring authoritative clarification

The following questions are formulated for clarification. **They are not
answered here.**

1. Does the applicable license permit use for **private personal self-hosting**
   (an individual running MusicPack for their own library)?
2. Does it permit **personal, non-commercial use** more broadly?
3. Does it permit **internal business use** (e.g. a company running MusicPack
   internally for its own staff)?
4. Does it permit **commercial business use**?
5. Does it permit **hosted MusicPack services** (where a third party hosts
   MusicPack for end users)?
6. Does **redistribution of MusicPack software without model weights** (i.e.
   the BSD-3-Clause code, which contains no model) implicate the model license
   at all?
7. Does **redistribution of model weights** require a separate licence, and if
   so under what terms?
8. Does the act of **running the model locally to produce embeddings**
   constitute a "use" restricted by NC?

---

## 5. G-2 — generated embeddings

**Gate question (ADR 0017 §10.4):** Do generated embeddings inherit the model's
NC/SA terms, and may a `.mpack` carrying them be redistributed?

### 5.1 Evidence

- **[License text]** Both candidate licenses define **Adapted Material** as
  material "derived from or based upon the Licensed Material and in which the
  Licensed Material is translated, altered, arranged, transformed, or
  otherwise modified." Embeddings are *outputs* of running the model, not
  modifications of the weights; whether they are Adapted Material is not
  addressed by the license text.
- **[License text]** ND §2(a)(2): "produce and reproduce, but **not Share**,
  Adapted Material for NonCommercial purposes only." SA §2(a)(2): "produce,
  reproduce, **and Share** Adapted Material for NonCommercial purposes only,"
  subject to ShareAlike.
- **[License text]** Both licenses, §2(a)(4): making technical modifications
  necessary to exercise the Licensed Rights never produces Adapted Material.
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

### 5.2 Questions requiring authoritative clarification

The following questions are formulated for clarification. **They are not
answered here.**

1. Are ordinary numerical **embedding vectors** generated from user-provided
   audio considered **outputs subject to the model license**, or are they
   unrestricted?
2. Does **storing vectors in a `.mpack`** similarity document constitute a
   use or adaptation requiring permission?
3. May **`.mpack` files containing vectors be redistributed** (shared, uploaded,
   sold)?
4. Are **server-local vectors** (never leaving the machine) treated differently
   from exported/redistributed vectors?
5. Are embeddings considered **adaptations/derivatives** of the model?
6. Do **attribution or ShareAlike obligations** apply to embeddings, and if so
   how are they satisfied?

---

## 6. G-3 — training data / Discogs provenance

**Gate question (ADR 0017 §10.4):** Are the Discogs research dataset terms
compatible with the artefact's NC/SA terms?

### 6.1 What the paper and official documentation establish

- **[Creator statement]** The model metadata records the dataset as
  "Discogs-4M (unreleased)", citation "In-house dataset", "4M full tracks"
  (3.3M used for `multi`, 1.7M used for `release`). The dataset is **not
  publicly released**.
- **[Creator statement]** The model paper (Alonso-Jiménez, Serra & Bogdanov,
  ISMIR 2022) states: "Discogs publishes monthly dumps of their data under the
  Creative Commons **CC0** license that we use to label 3.3 million tracks from
  our in-house data collection."
- **[Creator statement]** Discogs' own current data page
  (`data.discogs.com`) states: "This data is made available under the **CC0 No
  Rights Reserved license**." Discogs' API Terms of Use distinguish "CC0 Data"
  from "Restricted Data" and state that "CC0 Data is made available under the
  CC0 No Rights Reserved license." Discogs' Terms of Service state that
  "Content made available via our Data Dump is subject to the licenses found on
  the Data Dump landing page."
- **[Technical fact]** The training data is therefore two-layered: (a) Discogs
  editorial *metadata* (stated CC0 by both the creators' paper and Discogs'
  own current documentation) and (b) an **in-house audio collection** whose
  terms are not publicly documented.

### 6.2 What remains unknown

- **[Unresolved]** The **in-house audio collection's terms are unknown.** The
  dataset is unreleased, so its terms cannot be audited from public sources.
- **[Unresolved]** Whether any restriction in the training data (audio or
  metadata) "flows through" to the model artifact, or whether the model's NC
  term is independent of the training data's terms.
- **[Unresolved]** Whether the model's NC license is itself consistent with
  training on CC0-licensed metadata.

### 6.3 Questions requiring authoritative clarification

1. What are the **terms of the in-house audio collection** used for training?
2. Do Discogs' **current** terms (the paper cites CC0 monthly dumps) permit the
   use made of them, and has Discogs' position changed since 2022?
3. Does any restriction in the training data (audio or metadata) flow through
   to the model artifact, creating obligations for downstream users?
4. Is the model's NC license consistent with training on CC0 metadata?

**Note:** the fact that the metadata dump was stated CC0 does **not** by itself
establish that the model artifact, or the audio it was trained on, is free of
restrictions. No rights are inferred from the CC0 metadata alone.

---

## 7. G-4 — model artifact / ONNX / runtime

**Gate question (ADR 0017 §10.4):** Does using an ONNX graph through a
pure-Rust runtime satisfy the producing project's terms, without the
AGPL-oriented library?

### 7.1 Separating the layers

| Layer | What can be established | Tag |
| --- | --- | --- |
| **A. Original model / license** | The model is `EffnetDiscogs` v1 (2022-06-15), TensorFlow 2.8.0. Its license is **disputed** (§2.1). | [Creator statement] + [Unresolved] |
| **B. Exact ONNX artifact** | MusicPack uses the **creator-distributed** `.onnx` files (file sizes match the official directory; §3.1). MusicPack performs **no conversion**. | [Technical fact] |
| **C. Conversion to ONNX** | The ONNX files are the **licensor's own distribution format** (the `.pb` and `.onnx` files are both in the official directory; metadata lists both model types). MusicPack does not convert. | [Technical fact] |
| **D. rten runtime** | `rten 0.26.0`, MIT OR Apache-2.0 (ADR 0017 §10.1). Governs the **runtime only**; does **not** resolve the model-license question. | [Technical fact] + [Legal interpretation] |
| **E. MusicPack's own code** | BSD-3-Clause. Contains no model weights and no third-party model terms. | [Technical fact] |

### 7.2 The key question (formulated, not answered)

> Does conversion of the model into ONNX constitute an adaptation/modification
> permitted by the applicable license, and may the resulting ONNX artifact be
> used for the intended MusicPack scenarios?

**What the evidence supports:**

- **[License text]** Both candidate licenses, §2(a)(4): making technical
  modifications necessary to exercise the Licensed Rights never produces
  Adapted Material. Format conversion is the kind of technical modification
  this clause addresses.
- **[Technical fact]** The ONNX conversion was performed by the **licensor**
  (MTG), not by MusicPack. MusicPack consumes the official ONNX artifact as
  distributed.
- **[Legal interpretation]** rten's MIT/Apache-2.0 license governs the *runtime*
  only. It does **not** resolve the *model*-license question; the model's
  terms apply to the model regardless of which runtime executes it.
- **[Creator statement]** The licensor distributes the ONNX artifact publicly,
  which is evidence they intend it to be used, but the NC/SA/ND terms still
  apply to that use.

**What remains uncertain:** whether the producing project's terms (whichever
license governs) permit the intended MusicPack use — local inference, optional
`.mpack` carriage, and server-side indexing — and whether the ONNX artifact
carries the same terms as the `.pb artifact. **[Unresolved]**

### 7.3 Questions requiring authoritative clarification

1. Does the applicable license permit **ONNX conversion** of the model?
2. Does it permit **use of the ONNX artifact through a third-party runtime**
   (rten), without the AGPL-oriented Essentia library?
3. Does the ONNX artifact carry the **same license terms** as the original
   `.pb` artifact, or different ones?
4. Does the producing project's distribution of the ONNX artifact grant
   downstream users (MusicPack operators) the right to use it in the ways
   MusicPack enables?

---

## 8. Commercial license

### 8.1 What the official materials establish

- **[Creator statement]** MTG's industry/technology-transfer page
  (`upf.edu/web/mtg/tech-transfer`) presents a "Technology portfolio for
  licensing" and invites users to "Discover our technologies available for
  commercial use."
- **[Creator statement]** The Essentia entry in that portfolio is described as:
  "Software library and **AI models** containing an extensive collection of
  reusable algorithms for audio and music analysis, description, and synthesis.
  Designed to support large-scale industrial applications." This indicates the
  commercial licensing covers **AI models**, not only the software library.
- **[Creator statement]** The Essentia licensing page states the models "are
  also available under a **proprietary license upon request**," and directs
  licensing inquiries to the contact on that page.
- **[Creator statement]** The MTG industry page states: "For information about
  licensing conditions of the technologies or for any other technology
  transfer information please contact us," and provides a contact mechanism.

### 8.2 What can and cannot be established

| Question | Answer | Basis |
| --- | --- | --- |
| Does a separate commercial/proprietary license exist? | **Yes** — MTG offers a proprietary license upon request and a commercial technology portfolio covering AI models. | [Creator statement] |
| Who grants it? | MTG / Universitat Pompeu Fabra. | [Creator statement] |
| Does it cover model weights? | The portfolio describes Essentia as "Software library **and AI models**," indicating coverage of models; exact scope not public. | [Creator statement] |
| Does it cover ONNX conversion? | **Not established.** | [Unresolved] |
| Does it address embeddings/outputs? | **Not established.** | [Unresolved] |
| Is pricing/contact information public? | **No.** Contact required; no public pricing. | [Creator statement] |

**Conclusion:** a commercial/proprietary license exists and appears to cover AI
models, but its specific scope (weights, ONNX, embeddings) and terms are not
publicly established. **[Unresolved]** The contact shown on the official
licensing page is the mechanism to obtain it.

---

## 9. Clarification questions for MTG/UPF

The following questions are drafted to be specific enough that a written answer
can close G-1 through G-4. They are neutral and do not lead the recipient
toward a desired answer.

1. **Which license governs** the exact Discogs-EffNet artifacts used by
   MusicPack (`discogs_multi_embeddings-effnet-bs64-1.onnx` and
   `discogs_release_embeddings-effnet-bs64-1.onnx`)?
2. **Why do the official materials identify both CC BY-NC-SA 4.0 and CC
   BY-NC-ND 4.0** for these models, and which is correct?
3. Does the applicable license permit **ONNX conversion** of the model?
4. Does it permit use for **private self-hosted applications**?
5. Does it permit **internal business use**?
6. Does it permit **commercial/hosted use**?
7. What **license, if any, applies to embeddings** generated from user-provided
   audio?
8. May those embeddings be **redistributed inside a `.mpack` file**?
9. Do embeddings trigger **attribution, NonCommercial, or ShareAlike**
   obligations?
10. Are there **additional restrictions arising from the training data** that
    affect downstream users?
11. Is there a **commercial license** covering these uses, and what are its
    terms?
12. Can MTG/UPF provide **written confirmation** that MusicPack can rely upon
    for these scenarios?

---

## 10. Proposed clarification request

The following is a draft request MusicPack could send to MTG/UPF. It uses the
contact information provided on the official licensing page; no email address
is invented.

---

**To:** Music Technology Group (MTG), Universitat Pompeu Fabra — via the
contact information on the official Essentia licensing page
(`essentia.upf.edu/licensing_information.html`)

**Subject:** Licensing clarification request — Discogs-EffNet use in MusicPack

Dear Music Technology Group,

I am writing on behalf of MusicPack, an open-source (BSD-3-Clause), self-hosted
music player and library manager. MusicPack optionally integrates the
Discogs-EffNet music-embedding model to provide a "similar tracks"
capability. We are seeking written clarification of the licensing terms that
apply to our use, because the official materials appear to identify two
different Creative Commons licenses.

**The exact artifacts we use:**

- `discogs_multi_embeddings-effnet-bs64-1.onnx` — SHA-256
  `65cfde30655a939de420e5c09a49a43648336fdbbf79eb3af6d3b40176339d8e`
- `discogs_release_embeddings-effnet-bs64-1.onnx` — SHA-256
  `fb49bd4e906624bbc5ee09e648e88d53b28c23870a8a6bed145624fd062530e7`

**How we use the model:**

MusicPack uses the model to generate embedding vectors from user-owned audio.
The model weights are operator-supplied and are never bundled, downloaded, or
redistributed by MusicPack. Inference runs locally through the pure-Rust `rten`
runtime; the server performs no inference. Generated vectors are optionally
carried in a `.mpack` similarity document and indexed in a server-local
similarity index. We do not redistribute the model weights by default.

**Intended scenarios:** we seek clarity for both private/personal
self-hosted use and potential future commercial/hosted use.

**The licensing conflict we have observed:**

The official materials identify two different licenses for these models:

- `essentia.upf.edu/models.html` states: "All the models created by the MTG are
  licensed under CC BY-NC-SA 4.0."
- `essentia.upf.edu/licensing_information.html` states: "All the models are
  available under the CC BY-NC-ND 4.0 license."
- The `models/LICENSE` file names CC BY-NC-ND 4.0 in its header, but its
  human-readable summary describes adaptation rights and links to the CC
  BY-NC-SA 4.0 legal code.
- The model metadata JSON files contain no license field.

**Questions:**

1. Which license governs the exact artifacts listed above?
2. Why do the official materials identify both CC BY-NC-SA 4.0 and CC BY-NC-ND
   4.0, and which is correct?
3. Does the applicable license permit ONNX conversion of the model?
4. Does it permit use for private self-hosted applications?
5. Does it permit internal business use?
6. Does it permit commercial/hosted use?
7. What license, if any, applies to embeddings generated from user-provided
   audio?
8. May those embeddings be redistributed inside a `.mpack` file?
9. Do embeddings trigger attribution, NonCommercial, or ShareAlike obligations?
10. Are there additional restrictions arising from the training data that affect
    downstream users?
11. Is there a commercial license covering these uses, and what are its terms?
12. Can MTG/UPF provide written confirmation that MusicPack can rely upon for
    these scenarios?

We would be grateful for a written clarification at your convenience. Thank you
for your time and for making these models available.

Regards,
The MusicPack project

---

## 11. Gate mapping

| Gate | Question | Evidence currently available | Remaining blocker | What answer would close it |
| ---- | -------- | ---------------------------- | ----------------- | -------------------------- |
| **G-1** permitted use | What does "non-commercial" mean for self-hosted personal, business, and hosted use? | NC definition is intention-based and scenario-agnostic; MusicPack's code is BSD-3-Clause; weights are operator-supplied. | SA/ND conflict; NC interpretation is fact-specific and scenario-dependent. | A written MTG/UPF determination of the governing license **and** its NC interpretation for each scenario, confirmed by qualified legal review. |
| **G-2** embeddings | Do embeddings inherit NC/SA terms; may a `.mpack` carrying them be redistributed? | Embeddings are outputs, not the weights; Adapted Material definition is modification-based; no MTG statement on outputs. | SA/ND conflict; whether embeddings are Adapted Material has no precedent; `.mpak` redistribution question unanswered. | A written MTG/UPF determination of the license status of model outputs and whether a `.mpack` carrying them may be redistributed, confirmed by qualified legal review. |
| **G-3** training data | Are the Discogs dataset terms compatible with the artefact's NC/SA terms? | Metadata stated CC0 by the paper and by Discogs' own current data page; audio collection is unreleased/unknown. | Audio collection terms unknown; flow-through question unanswered. | Disclosure of the in-house audio collection's terms, confirmation of Discogs' current metadata license, and a qualified legal opinion on compatibility. |
| **G-4** ONNX / runtime | Does ONNX use through rten satisfy the producing project's terms? | ONNX files are creator-distributed; MusicPack performs no conversion; rten is MIT/Apache-2.0; Essentia library not used. | SA/ND conflict; whether ONNX use through a third-party runtime is permitted; proprietary-license terms unknown. | A written MTG/UPF determination of the governing license and permission for ONNX use through a third-party runtime, plus the proprietary-license terms, confirmed by qualified legal review. |

**Conservative reading:** no gate is closable on the current evidence. Each
gate requires (a) resolution of the SA/ND conflict (G-1, G-2, G-4) and/or
disclosure of non-public terms (G-3), and (b) qualified legal review. The
evidence needed to close each gate is identified above; it does not yet exist.

---

## 12. Recommended next step

1. **Obtain written clarification from MTG/UPF** using the questions in §9 and
   the request in §10. This is the single highest-priority step, as the SA/ND
   conflict is a threshold blocker for G-1, G-2, and G-4.
2. **Separately verify current Discogs data-dump licensing** against Discogs'
   own current documentation (independent of the creators' paper), to confirm
   the metadata layer's CC0 status and to identify any Restricted Data
   carve-outs.
3. **Obtain qualified legal review** where required — specifically for the NC
   interpretation (G-1), the Adapted Material / redistribution question (G-2),
   training-data compatibility (G-3), and ONNX/runtime use (G-4).
4. **Only then update the gate decisions** in ADR 0017 §10.4 and
   `docs/licensing-g1-g4-readiness.md`.

No production changes are recommended at this stage.

---

## Sources consulted (external, accessed 2026-10-01/02)

- Essentia models page — https://essentia.upf.edu/models.html
- Essentia licensing page — https://essentia.upf.edu/licensing_information.html
- Essentia models LICENSE — https://essentia.upf.edu/models/LICENSE
- Model metadata JSON (multi) — https://essentia.upf.edu/models/feature-extractors/discogs-effnet/discogs_multi_embeddings-effnet-bs64-1.json
- Model metadata JSON (release) — https://essentia.upf.edu/models/feature-extractors/discogs-effnet/discogs_release_embeddings-effnet-bs64-1.json
- Model directory listing — https://essentia.upf.edu/models/feature-extractors/discogs-effnet/
- MTG industry / technology-transfer page — https://www.upf.edu/web/mtg/tech-transfer
- CC BY-NC-ND 4.0 legal code — https://creativecommons.org/licenses/by-nc-nd/4.0/legalcode
- CC BY-NC-SA 4.0 legal code — https://creativecommons.org/licenses/by-nc-sa/4.0/legalcode
- Model paper — Alonso-Jiménez, Serra & Bogdanov, "Music Representation Learning Based on Editorial Metadata from Discogs," ISMIR 2022 — https://repositori.upf.edu/handle/10230/54473
- Discogs data page — https://data.discogs.com/
- Discogs API Terms of Use — https://support.discogs.com/hc/en-us/articles/360009334593
- Discogs Terms of Service — https://support.discogs.com/hc/en-us/articles/360009334333

**Distinction:** the external research above is recorded as evidence with its
source. It is separate from, and does not rewrite, MusicPack's own established
findings in `docs/licensing-g1-g4-readiness.md` and ADR 0017 §10. Where the
external sources and the MusicPack record agree (e.g. the SA/ND conflict), the
agreement is noted; where they differ, the difference is preserved.
