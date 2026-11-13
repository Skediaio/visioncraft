// Trunk hooks: download progress while the wasm loads, and a readable message if loading fails.
export default function initializer() {
  return {
    onStart: () => window.vcLoading(window.vcT.loading, 0.02),
    onProgress: ({ current, total }) => {
      if (total) window.vcLoading(window.vcT.loading, current / total);
    },
    onSuccess: () => window.vcLoading(window.vcT.starting, 1),
    onFailure: (error) => window.vcFail(window.vcT.failedTitle, String(error)),
  };
}
