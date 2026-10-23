// fork: the screen where the user reviews Home AI suggestions: albums to
// create, files to clean up and files to hide. Nothing happens to a file
// until the user picks it and confirms.

import { useWrapAsyncOperation } from "@/components/utils/use-wrap-async";
import {
    Alert,
    Box,
    Checkbox,
    Dialog,
    DialogContent,
    DialogTitle,
    Stack,
    styled,
    Tab,
    Tabs,
    Typography,
} from "@mui/material";
import { Overlay } from "ente-base/components/containers";
import { ActivityIndicator } from "ente-base/components/mui/ActivityIndicator";
import { DialogCloseIconButton } from "ente-base/components/mui/DialogCloseIconButton";
import { FocusVisibleButton } from "ente-base/components/mui/FocusVisibleButton";
import { useBaseContext } from "ente-base/context";
import type { EnteFile } from "ente-media/file";
import { ItemCard } from "ente-new/photos/components/Tiles";
import {
    homeAISnapshot,
    homeAISubscribe,
    homeAISuggestions,
} from "ente-new/photos/services/fork";
import {
    acceptAlbumSuggestion,
    dismissHideSuggestions,
    hideSuggestedFiles,
    keepCleanupSuggestions,
    rejectAlbumSuggestion,
    trashCleanupSuggestions,
} from "ente-new/photos/services/fork/review";
import type {
    AlbumSuggestion,
    HomeAISuggestions,
    ScoredFile,
} from "ente-new/photos/services/fork/suggestions";
import React, {
    useCallback,
    useEffect,
    useState,
    useSyncExternalStore,
} from "react";
import { homeAIReasonLabel } from "./home-ai-labels";
import {
    hideHomeAIReview,
    homeAIReviewOpenSnapshot,
    homeAIReviewOpenSubscribe,
} from "./home-ai-review-store";

interface HomeAIReviewHostProps {
    /** Pull remote changes so the gallery and the suggestions update. */
    onRemoteFilesPull: () => Promise<void>;
}

/** Mounted once in the gallery; shows the review screen when asked. */
export const HomeAIReviewHost: React.FC<HomeAIReviewHostProps> = ({
    onRemoteFilesPull,
}) => {
    const open = useSyncExternalStore(
        homeAIReviewOpenSubscribe,
        homeAIReviewOpenSnapshot,
        homeAIReviewOpenSnapshot,
    );
    return open ? (
        <HomeAIReview
            onClose={hideHomeAIReview}
            onRemoteFilesPull={onRemoteFilesPull}
        />
    ) : null;
};

type ReviewTab = "albums" | "cleanup" | "hide";

/** Long lists are reviewed in pages of this size, best matches first. */
const pageSize = 120;

const HomeAIReview: React.FC<
    HomeAIReviewHostProps & { onClose: () => void }
> = ({ onClose, onRemoteFilesPull }) => {
    const state = useSyncExternalStore(homeAISubscribe, homeAISnapshot);
    const [tab, setTab] = useState<ReviewTab>("albums");
    const [suggestions, setSuggestions] = useState<HomeAISuggestions>();

    const refresh = useCallback(
        () => void homeAISuggestions().then(setSuggestions),
        [],
    );

    useEffect(refresh, [refresh, state?.resultCount]);

    const afterAction = useCallback(async () => {
        await onRemoteFilesPull();
        refresh();
    }, [onRemoteFilesPull, refresh]);

    return (
        <Dialog open fullScreen onClose={onClose}>
            <DialogTitle>
                <Stack
                    direction="row"
                    sx={{
                        alignItems: "center",
                        justifyContent: "space-between",
                    }}
                >
                    <Typography variant="h3">Home AI suggestions</Typography>
                    <DialogCloseIconButton onClose={onClose} />
                </Stack>
            </DialogTitle>
            <DialogContent>
                <Stack sx={{ gap: 2 }}>
                    {state?.hasSamples && (
                        <Alert severity="warning">
                            Some suggestions are sample data, made up on this
                            device. Actions you confirm still apply to the real
                            photos.
                        </Alert>
                    )}
                    <Tabs value={tab} onChange={(_, t: ReviewTab) => setTab(t)}>
                        <Tab
                            value="albums"
                            label={`Albums (${suggestions?.albums.length ?? 0})`}
                        />
                        <Tab
                            value="cleanup"
                            label={`Cleanup (${suggestions?.cleanup.length ?? 0})`}
                        />
                        <Tab
                            value="hide"
                            label={`Hide (${suggestions?.hide.length ?? 0})`}
                        />
                    </Tabs>
                    {!suggestions ? (
                        <Box sx={{ textAlign: "center", pt: 4 }}>
                            <ActivityIndicator />
                        </Box>
                    ) : tab == "albums" ? (
                        <AlbumSuggestions
                            albums={suggestions.albums}
                            onDidAct={afterAction}
                        />
                    ) : tab == "cleanup" ? (
                        <CleanupSuggestions
                            items={suggestions.cleanup}
                            onDidAct={afterAction}
                        />
                    ) : (
                        <HideSuggestions
                            items={suggestions.hide}
                            onDidAct={afterAction}
                        />
                    )}
                </Stack>
            </DialogContent>
        </Dialog>
    );
};

interface ActProps {
    onDidAct: () => Promise<void>;
}

const Empty: React.FC<{ children: React.ReactNode }> = ({ children }) => (
    <Typography sx={{ color: "text.muted", py: 4, textAlign: "center" }}>
        {children}
    </Typography>
);

const AlbumSuggestions: React.FC<ActProps & { albums: AlbumSuggestion[] }> = ({
    albums,
    onDidAct,
}) =>
    albums.length == 0 ? (
        <Empty>No album suggestions right now.</Empty>
    ) : (
        <Stack sx={{ gap: 4 }}>
            {albums.map((album) => (
                <AlbumSuggestionRow
                    key={album.id}
                    album={album}
                    onDidAct={onDidAct}
                />
            ))}
        </Stack>
    );

const AlbumSuggestionRow: React.FC<ActProps & { album: AlbumSuggestion }> = ({
    album,
    onDidAct,
}) => {
    const [selected, setSelected] = useState(
        () => new Set(album.files.map((f) => f.id)),
    );
    const selectedFiles = album.files.filter((f) => selected.has(f.id));

    const accept = useWrapAsyncOperation(async () => {
        await acceptAlbumSuggestion(album, selectedFiles);
        await onDidAct();
    });
    const reject = useWrapAsyncOperation(async () => {
        await rejectAlbumSuggestion(album.files);
        await onDidAct();
    });

    const isExisting = album.existingCollectionID !== undefined;
    return (
        <Stack sx={{ gap: 1 }}>
            <Stack
                direction="row"
                sx={{ alignItems: "center", gap: 2, flexWrap: "wrap" }}
            >
                <Stack sx={{ flex: 1, minWidth: 160 }}>
                    <Typography sx={{ fontWeight: "medium" }}>
                        {album.name}
                    </Typography>
                    <Typography variant="small" sx={{ color: "text.muted" }}>
                        {isExisting
                            ? `${album.files.length} photos that may belong in your album`
                            : `${album.files.length} photos for a new album`}
                    </Typography>
                </Stack>
                <FocusVisibleButton
                    color="accent"
                    disabled={selectedFiles.length == 0}
                    onClick={() => void accept()}
                >
                    {isExisting
                        ? `Add ${selectedFiles.length}`
                        : `Create with ${selectedFiles.length}`}
                </FocusVisibleButton>
                <FocusVisibleButton
                    color="secondary"
                    onClick={() => void reject()}
                >
                    Not this album
                </FocusVisibleButton>
            </Stack>
            <FileGrid
                items={album.files.map((file) => ({ file, labels: [] }))}
                selected={selected}
                onSelect={setSelected}
            />
        </Stack>
    );
};

const CleanupSuggestions: React.FC<ActProps & { items: ScoredFile[] }> = ({
    items,
    onDidAct,
}) => {
    const { showMiniDialog } = useBaseContext();
    const page = items.slice(0, pageSize);
    const [selected, setSelected] = useState(new Set<number>());
    const selectedFiles = page
        .filter((i) => selected.has(i.file.id))
        .map((i) => i.file);

    const done = useCallback(async () => {
        setSelected(new Set());
        await onDidAct();
    }, [onDidAct]);

    const keep = useWrapAsyncOperation(async () => {
        await keepCleanupSuggestions(selectedFiles);
        await done();
    });

    const confirmTrash = () =>
        showMiniDialog({
            title: `Move ${selectedFiles.length} to trash?`,
            message:
                "They stay in trash for 30 days, and you can restore them from there.",
            continue: {
                text: "Move to trash",
                color: "critical",
                action: async () => {
                    await trashCleanupSuggestions(selectedFiles);
                    await done();
                },
            },
        });

    if (items.length == 0) return <Empty>Nothing to clean up right now.</Empty>;

    return (
        <Stack sx={{ gap: 1 }}>
            <SelectionBar
                count={items.length}
                shown={page.length}
                selected={selectedFiles.length}
                onSelectAll={() =>
                    setSelected(new Set(page.map((i) => i.file.id)))
                }
                onSelectNone={() => setSelected(new Set())}
            >
                <FocusVisibleButton
                    color="critical"
                    disabled={selectedFiles.length == 0}
                    onClick={confirmTrash}
                >
                    Move to trash
                </FocusVisibleButton>
                <FocusVisibleButton
                    color="secondary"
                    disabled={selectedFiles.length == 0}
                    onClick={() => void keep()}
                >
                    Keep
                </FocusVisibleButton>
            </SelectionBar>
            <FileGrid items={page} selected={selected} onSelect={setSelected} />
        </Stack>
    );
};

const HideSuggestions: React.FC<ActProps & { items: ScoredFile[] }> = ({
    items,
    onDidAct,
}) => {
    const page = items.slice(0, pageSize);
    const [selected, setSelected] = useState(new Set<number>());
    const selectedFiles = page
        .filter((i) => selected.has(i.file.id))
        .map((i) => i.file);

    const act = useWrapAsyncOperation(
        async (op: (files: EnteFile[]) => Promise<unknown>) => {
            await op(selectedFiles);
            setSelected(new Set());
            await onDidAct();
        },
    );

    if (items.length == 0) {
        return <Empty>No photos that look sensitive right now.</Empty>;
    }

    return (
        <Stack sx={{ gap: 1 }}>
            <Typography variant="small" sx={{ color: "text.muted" }}>
                These may show ID documents, card numbers or other personal
                data. Hidden photos sit behind your device lock and leave shared
                albums.
            </Typography>
            <SelectionBar
                count={items.length}
                shown={page.length}
                selected={selectedFiles.length}
                onSelectAll={() =>
                    setSelected(new Set(page.map((i) => i.file.id)))
                }
                onSelectNone={() => setSelected(new Set())}
            >
                <FocusVisibleButton
                    color="accent"
                    disabled={selectedFiles.length == 0}
                    onClick={() => void act(hideSuggestedFiles)}
                >
                    Hide
                </FocusVisibleButton>
                <FocusVisibleButton
                    color="secondary"
                    disabled={selectedFiles.length == 0}
                    onClick={() => void act(dismissHideSuggestions)}
                >
                    Not sensitive
                </FocusVisibleButton>
            </SelectionBar>
            <FileGrid items={page} selected={selected} onSelect={setSelected} />
        </Stack>
    );
};

interface SelectionBarProps {
    count: number;
    shown: number;
    selected: number;
    onSelectAll: () => void;
    onSelectNone: () => void;
    children: React.ReactNode;
}

const SelectionBar: React.FC<SelectionBarProps> = ({
    count,
    shown,
    selected,
    onSelectAll,
    onSelectNone,
    children,
}) => (
    <Stack
        direction="row"
        sx={{ alignItems: "center", gap: 1, flexWrap: "wrap" }}
    >
        <Typography sx={{ flex: 1, minWidth: 160, color: "text.muted" }}>
            {shown < count
                ? `${selected} selected. Showing the ${shown} likeliest of ${count}.`
                : `${selected} of ${count} selected`}
        </Typography>
        <FocusVisibleButton color="secondary" onClick={onSelectAll}>
            Select all
        </FocusVisibleButton>
        <FocusVisibleButton color="secondary" onClick={onSelectNone}>
            Select none
        </FocusVisibleButton>
        {children}
    </Stack>
);

interface FileGridProps {
    items: { file: EnteFile; labels: string[] }[];
    selected: Set<number>;
    onSelect: (selected: Set<number>) => void;
}

const FileGrid: React.FC<FileGridProps> = ({ items, selected, onSelect }) => {
    const toggle = (id: number) => {
        const next = new Set(selected);
        if (!next.delete(id)) next.add(id);
        onSelect(next);
    };
    return (
        <Box
            sx={{
                display: "grid",
                gridTemplateColumns: "repeat(auto-fill, minmax(112px, 1fr))",
                gap: 0.5,
            }}
        >
            {items.map(({ file, labels }) => (
                <Box
                    key={file.id}
                    sx={{ position: "relative", aspectRatio: "1" }}
                >
                    <ItemCard
                        TileComponent={GridTile}
                        coverFile={file}
                        onClick={() => toggle(file.id)}
                    >
                        {labels.length > 0 && (
                            <LabelOverlay>
                                <Typography variant="mini">
                                    {labels.map(homeAIReasonLabel).join(", ")}
                                </Typography>
                            </LabelOverlay>
                        )}
                    </ItemCard>
                    <Checkbox
                        size="small"
                        checked={selected.has(file.id)}
                        onChange={() => toggle(file.id)}
                        slotProps={{
                            input: { "aria-label": file.metadata.title },
                        }}
                        sx={{
                            position: "absolute",
                            top: 0,
                            left: 0,
                            color: "white",
                            "&.Mui-checked": { color: "white" },
                        }}
                    />
                </Box>
            ))}
        </Box>
    );
};

const GridTile = styled("div")`
    display: flex;
    position: relative;
    width: 100%;
    height: 100%;
    border-radius: 4px;
    overflow: hidden;
    cursor: pointer;
    & > img {
        object-fit: cover;
        width: 100%;
        height: 100%;
        pointer-events: none;
    }
`;

const LabelOverlay = styled(Overlay)`
    display: flex;
    align-items: flex-end;
    padding: 4px 6px;
    color: white;
    background: linear-gradient(
        to top,
        rgba(0, 0, 0, 0.6) 0%,
        rgba(0, 0, 0, 0) 50%
    );
`;
