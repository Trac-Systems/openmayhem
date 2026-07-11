export const dispatchContainedClientRequest = (dispatch, onError) => {
  try {
    const pending = dispatch();
    if (pending && typeof pending.then === 'function') {
      Promise.resolve(pending).catch(onError);
    }
  } catch (error) {
    onError(error);
  }
};
