export const load = ({ url }: { url: URL }) => ({
	share: url.searchParams.get('share')
});
