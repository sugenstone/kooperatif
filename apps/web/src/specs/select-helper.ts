import { screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

/**
 * Drive a shadcn-svelte (bits-ui) Select in jsdom: open the trigger and
 * pick the option by its accessible name. The trigger is resolved by
 * its DOM id (kept stable for tests) or, via `byLabel`, by the label
 * text wired through aria-labelledby.
 */
export async function pickSelectOption(
	target: string | HTMLElement,
	optionName: string | RegExp
): Promise<void> {
	const trigger = typeof target === 'string' ? document.getElementById(target) : target;
	if (!trigger) throw new Error(`select trigger not found: ${String(target)}`);
	await waitForUnlockedBody();
	await userEvent.click(trigger);
	// floating-ui's `hide` middleware parks the content offscreen with
	// visibility:hidden in jsdom (no real layout), so `hidden: true` is
	// required — and the accessible name is matched against textContent
	// manually because hidden options compute no reliable name.
	const options = await screen.findAllByRole('option', { hidden: true });
	const match = options.find((o) =>
		typeof optionName === 'string'
			? (o.textContent ?? '').trim() === optionName
			: optionName.test(o.textContent ?? '')
	);
	if (!match) {
		throw new Error(
			`select option not found: ${String(optionName)} — got: ${options
				.map((o) => (o.textContent ?? '').trim())
				.join(' | ')}`
		);
	}
	await userEvent.click(match);
	// The open layer scroll-locks <body> (pointer-events: none); the lock is
	// released on the microtask/effect after close — wait for it so the next
	// interaction does not race the unlock.
	await waitForUnlockedBody();
}

export async function waitForUnlockedBody(): Promise<void> {
	await waitFor(() => {
		if (document.body.style.pointerEvents === 'none') {
			throw new Error('select layer still locks pointer events');
		}
	});
}

export async function pickSelectOptionByLabel(
	label: string | RegExp,
	optionName: string | RegExp
): Promise<void> {
	await pickSelectOption(await screen.findByLabelText(label), optionName);
}
