export const load = ({ url }: { url: URL }) => ({
	family: url.searchParams.get('family')
});
