// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// https://astro.build/config
export default defineConfig({
	site: 'https://bugatti.dev',
	integrations: [
		starlight({
			title: 'bugatti',
			social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/codesoda/bugatti-cli' }],
			sidebar: [
				{ label: 'Getting Started', slug: 'getting-started' },
				{
					label: 'Guides',
					items: [
						{ label: 'Writing Tests', slug: 'guides/writing-tests' },
						{ label: 'Configuration', slug: 'guides/configuration' },
						{ label: 'Commands', slug: 'guides/commands' },
						{ label: 'Includes & Composition', slug: 'guides/includes' },
						{ label: 'Skipping Steps', slug: 'guides/skipping' },
						{ label: 'Checkpoints', slug: 'guides/checkpoints' },
						{ label: 'Test Discovery', slug: 'guides/test-discovery' },
						{ label: 'Per-Test Overrides', slug: 'guides/per-test-overrides' },
					],
				},
				{
					label: 'Reference',
					items: [
						{ label: 'CLI', slug: 'reference/cli' },
						{ label: 'Config File', slug: 'reference/config-file' },
						{ label: 'Test File', slug: 'reference/test-file' },
						{ label: 'Result Contract', slug: 'reference/result-contract' },
						{ label: 'Exit Codes', slug: 'reference/exit-codes' },
					],
				},
				{
					label: 'Examples',
					items: [
						{ label: 'Overview', slug: 'examples' },
						{ label: 'Static HTML', slug: 'examples/static-html' },
						{ label: 'Node + Express', slug: 'examples/node-express' },
						{ label: 'Python + Flask', slug: 'examples/python-flask' },
						{ label: 'Rust CLI', slug: 'examples/rust-cli' },
					],
				},
			],
		}),
	],
});
