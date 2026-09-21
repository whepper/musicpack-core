// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

import { mount } from 'svelte';
import App from './app.svelte';
import './lib/ui/theme.css';

mount(App, { target: document.getElementById('app')! });
