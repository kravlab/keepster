/** The SPA's entry: mounts the application shell into the page the daemon serves. */
import { mount } from 'svelte'
import App from './App.svelte'
import './app.css'

const target = document.getElementById('app')
if (!target) throw new Error('index.html has no #app element')
mount(App, { target })
