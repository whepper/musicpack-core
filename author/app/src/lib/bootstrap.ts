// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Composition root for the MusicPack Author frontend. Mirrors the web
// client's bootstrap pattern: singletons are constructed here and imported
// by components; the Svelte runtime is kept out of the plain-TS modules.
//
// Stores are exported as individual bindings (not via a destructured
// container) so the Svelte compiler can statically detect them for `$store`
// auto-subscription.

import { AuthorApi } from './api';
import { createDraftStore } from './draft-store';

export const api = new AuthorApi();

export const draftStore = createDraftStore();
export const draft = draftStore.draft;
export const busy = draftStore.busy;
export const error = draftStore.error;
