/* PROTOTYPE (wayfinder ticket 15) — entry for the timeline/restore UX variants. */
import { mount } from 'svelte'
import Host from './prototype-timeline/Host.svelte'
import './app.css'

mount(Host, { target: document.getElementById('app')! })
