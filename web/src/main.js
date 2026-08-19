import { mount } from 'svelte';
import { initTheme } from '@kenn-io/kit-ui';
import '@kenn-io/kit-ui/theme.css';
import './app.css';
import App from './App.svelte';

initTheme();
mount(App, { target: document.getElementById('app') });
