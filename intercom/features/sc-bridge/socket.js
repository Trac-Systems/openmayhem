export const closeBridgeSocket = (socket) => {
  try {
    socket?.end?.();
  } catch (_e) {}
  try {
    socket?.destroy?.();
  } catch (_e) {}
};
