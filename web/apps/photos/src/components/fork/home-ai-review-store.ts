// fork: whether the Home AI review screen is open. Kept outside the sidebar
// so the screen survives the sidebar closing.

let _open = false;
const _listeners = new Set<() => void>();

const setOpen = (open: boolean) => {
    _open = open;
    _listeners.forEach((l) => l());
};

// Open the review screen from anywhere, e.g. the settings drawer.
export const showHomeAIReview = () => setOpen(true);

export const hideHomeAIReview = () => setOpen(false);

export const homeAIReviewOpenSubscribe = (onChange: () => void) => {
    _listeners.add(onChange);
    return () => void _listeners.delete(onChange);
};

export const homeAIReviewOpenSnapshot = () => _open;
