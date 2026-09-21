// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

import { mount } from 'svelte';
import App from './app.svelte';
import './lib/ui/theme.css';
import { exposeDebug, offline, player, session } from './lib/bootstrap';
import { registerServiceWorker } from './lib/offline/register-sw';

// Offline hydration must complete BEFORE the session probe: the boot path
// consults the catalog to distinguish 'signed out' from 'offline but with
// installed content' (AuthState 'offline'). Both are async; sequence them.
void (async () => {
  await offline.init();
  await session.boot();
})();
player.init();
exposeDebug();
registerServiceWorker();

const target = document.getElementById('app');
if (!target) throw new Error('missing #app mount point');

mount(App, { target });
