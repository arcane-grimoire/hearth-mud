import { mount } from 'svelte';
import '@kenn-io/kit-ui/theme.css';
import './app.css';
import App from './App.svelte';

mount(App, { target: document.getElementById('app') });
