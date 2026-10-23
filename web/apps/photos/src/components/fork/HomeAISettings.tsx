// fork: the Home AI settings drawer. Turning Home AI on asks the user to
// compare the worker's fingerprint once; after that this device releases
// fileKeys to the worker during sync.

import { useWrapAsyncOperation } from "@/components/utils/use-wrap-async";
import { Box, Stack, Typography } from "@mui/material";
import { EnteSwitch } from "ente-base/components/EnteSwitch";
import { ActivityIndicator } from "ente-base/components/mui/ActivityIndicator";
import { FocusVisibleButton } from "ente-base/components/mui/FocusVisibleButton";
import {
    TitledNestedSidebarDrawer,
    type NestedSidebarDrawerVisibilityProps,
} from "ente-base/components/mui/SidebarDrawer";
import { RowButtonGroupHint } from "ente-base/components/RowButton";
import { RowCard } from "ente-base/components/v2/RowCard";
import { useBaseContext } from "ente-base/context";
import {
    clearHomeAISamples,
    disableHomeAI,
    enableHomeAI,
    homeAISnapshot,
    homeAISubscribe,
    homeAISync,
    loadHomeAISamples,
    pullHomeAIState,
    stopHomeAIOnThisDevice,
    updateHomeAISettings,
    type HomeAIState,
} from "ente-new/photos/services/fork";
import { workerFingerprint } from "ente-new/photos/services/fork/key-release";
import type { HomeAISettings as Settings } from "ente-new/photos/services/fork/suggestions";
import type { WorkerKey } from "ente-new/photos/services/fork/worker-api";
import React, { useEffect, useSyncExternalStore } from "react";
import { DropdownInput } from "../sidebar/DropdownInput";
import { showHomeAIReview } from "./home-ai-review-store";

export const HomeAISettings: React.FC<NestedSidebarDrawerVisibilityProps> = ({
    open,
    onClose,
    onRootClose,
}) => {
    const state = useSyncExternalStore(homeAISubscribe, homeAISnapshot);

    useEffect(() => {
        if (open) void pullHomeAIState();
    }, [open]);

    const handleRootClose = () => {
        onClose();
        onRootClose();
    };

    const handleReview = () => {
        handleRootClose();
        showHomeAIReview();
    };

    return (
        <TitledNestedSidebarDrawer
            maxWidth="440px"
            {...{ open, onClose }}
            onRootClose={handleRootClose}
            title="Home AI"
            caption="Suggestions from the AI worker on your own computer"
        >
            {!state ? (
                <Box sx={{ textAlign: "center", pt: 4 }}>
                    <ActivityIndicator />
                </Box>
            ) : !state.available ? (
                <Typography sx={{ color: "text.muted", px: 3, py: 2 }}>
                    This server does not support Home AI. It needs the
                    fork&apos;s museum with a worker key in{" "}
                    <code>fork.worker-keys</code>.
                </Typography>
            ) : (
                <Stack sx={{ px: 2, py: 1, gap: 3 }}>
                    <WorkerSection state={state} />
                    <ResultsSection state={state} onReview={handleReview} />
                    <PreferencesSection settings={state.settings} />
                    <SamplesSection state={state} />
                </Stack>
            )}
        </TitledNestedSidebarDrawer>
    );
};

const groupedFingerprint = (fingerprint: string) =>
    fingerprint.match(/.{1,4}/g)?.join(" ") ?? fingerprint;

const formatTime = (epochMicroseconds: number) =>
    new Date(epochMicroseconds / 1000).toLocaleString();

const WorkerSection: React.FC<{ state: HomeAIState }> = ({ state }) => {
    const { showMiniDialog } = useBaseContext();
    const { keys, workers, device } = state;

    const enable = useWrapAsyncOperation(enableHomeAI);
    const stopHere = useWrapAsyncOperation(stopHomeAIOnThisDevice);
    const killSwitch = useWrapAsyncOperation(disableHomeAI);

    const confirmKey = async (key: WorkerKey) => {
        const fingerprint = await workerFingerprint(key.publicKey);
        showMiniDialog({
            title: `Send photos to "${key.name}"?`,
            message: (
                <Stack sx={{ gap: 2 }}>
                    <Typography>
                        The worker will be able to open every photo and video
                        you own, so it can analyse them. Compare this
                        fingerprint with the one the worker prints when it
                        starts. Only continue if every character matches.
                    </Typography>
                    <Typography
                        component="code"
                        sx={{
                            fontFamily: "monospace",
                            wordBreak: "break-word",
                            p: 1.5,
                            borderRadius: 1,
                            bgcolor: "fill.faint",
                        }}
                    >
                        {groupedFingerprint(fingerprint)}
                    </Typography>
                </Stack>
            ),
            continue: {
                text: "It matches, turn on",
                action: () => enable(key, fingerprint),
            },
        });
    };

    const handleToggle = () => {
        if (device) {
            showMiniDialog({
                title: "Turn off Home AI?",
                message:
                    "Turning it off for your account revokes the worker's access at once and drops keys it has not read yet. Results it already made stay. Stopping on this device only keeps the worker going with keys from your other devices.",
                continue: {
                    text: "Turn off for my account",
                    color: "critical",
                    action: killSwitch,
                },
                secondary: { text: "Stop on this device", action: stopHere },
            });
        } else if (keys.length == 1) {
            void confirmKey(keys[0]!);
        }
    };

    if (keys.length == 0) {
        return (
            <Typography sx={{ color: "text.muted", px: 1 }}>
                No worker is on this server&apos;s allowlist yet. Add the
                worker&apos;s public key under <code>fork.worker-keys</code> in
                museum.yaml, then restart museum.
            </Typography>
        );
    }

    const status = workers.find((w) => w.workerPublicKey == device?.publicKey);
    const isOnElsewhere = !device && workers.length > 0;

    return (
        <Stack>
            <RowCard
                title="Analyse my library on Home AI"
                endIcon={
                    <EnteSwitch
                        checked={!!device}
                        disabled={!device && keys.length != 1}
                        onChange={handleToggle}
                        slotProps={{
                            input: {
                                "aria-label": "Analyse my library on Home AI",
                            },
                        }}
                    />
                }
            />
            {!device && keys.length > 1 && (
                <Stack sx={{ gap: 1, pt: 1 }}>
                    {keys.map((key) => (
                        <RowCard
                            key={key.publicKey}
                            title={key.name}
                            subtitle="Choose this worker"
                            onClick={() => void confirmKey(key)}
                        />
                    ))}
                </Stack>
            )}
            {isOnElsewhere && (
                <RowButtonGroupHint>
                    Home AI is on for your account from another device. Turn it
                    on here too to send keys from this device.
                </RowButtonGroupHint>
            )}
            {device && (
                <>
                    <RowCard
                        title={<Faint>Worker</Faint>}
                        endIcon={<Typography>{device.name}</Typography>}
                    />
                    <RowCard
                        title={<Faint>Sent, waiting for the worker</Faint>}
                        endIcon={
                            <Typography>{status?.pendingCount ?? 0}</Typography>
                        }
                    />
                    <RowCard
                        title={<Faint>Read by the worker</Faint>}
                        endIcon={
                            <Typography>
                                {status?.consumedCount ?? 0}
                            </Typography>
                        }
                    />
                    {status?.lastUsedAt && (
                        <RowCard
                            title={<Faint>Worker last seen</Faint>}
                            endIcon={
                                <Typography>
                                    {formatTime(status.lastUsedAt)}
                                </Typography>
                            }
                        />
                    )}
                    {state.lastRelease?.state == "done" &&
                        state.lastRelease.remaining > 0 && (
                            <RowButtonGroupHint>
                                {`${state.lastRelease.remaining} more files will be sent on the next syncs.`}
                            </RowButtonGroupHint>
                        )}
                </>
            )}
            {state.lastRelease?.state == "stopped" && (
                <RowButtonGroupHint>
                    {`Stopped sending keys: ${state.lastRelease.reason}`}
                </RowButtonGroupHint>
            )}
        </Stack>
    );
};

const Faint: React.FC<{ children: React.ReactNode }> = ({ children }) => (
    <Typography component="span" sx={{ color: "text.faint", pl: 1 }}>
        {children}
    </Typography>
);

const ResultsSection: React.FC<{
    state: HomeAIState;
    onReview: () => void;
}> = ({ state, onReview }) => {
    const sync = useWrapAsyncOperation(homeAISync);
    return (
        <Stack sx={{ gap: 1 }}>
            <RowCard
                title="Review suggestions"
                subtitle={`${state.resultCount} analysed photos on this device`}
                onClick={onReview}
            />
            <FocusVisibleButton color="secondary" onClick={() => void sync()}>
                Sync now
            </FocusVisibleButton>
        </Stack>
    );
};

const scoreOptions = [0.5, 0.6, 0.7, 0.8, 0.9].map((v) => ({
    label: `${Math.round(v * 100)}% sure`,
    value: v.toString(),
}));

const PreferencesSection: React.FC<{ settings: Settings }> = ({ settings }) => {
    const update = useWrapAsyncOperation(async (updates: Partial<Settings>) => {
        await updateHomeAISettings(updates);
    });

    const scoreRow = (
        title: string,
        key: "minAlbumScore" | "minCleanupScore" | "minPIIScore",
    ) => (
        <Stack sx={{ gap: 0.5 }}>
            <Typography variant="small" sx={{ color: "text.muted", px: 1 }}>
                {title}
            </Typography>
            <DropdownInput
                options={scoreOptions}
                selected={settings[key].toString()}
                onSelect={(v) => void update({ [key]: Number(v) })}
            />
        </Stack>
    );

    return (
        <Stack sx={{ gap: 1 }}>
            <Typography variant="small" sx={{ color: "text.muted", px: 1 }}>
                Settings on this device
            </Typography>
            <RowCard
                title="Show results in search"
                endIcon={
                    <EnteSwitch
                        checked={settings.search}
                        onChange={() =>
                            void update({ search: !settings.search })
                        }
                        slotProps={{
                            input: { "aria-label": "Show results in search" },
                        }}
                    />
                }
            />
            <Stack>
                <RowCard
                    title="Include hidden items"
                    endIcon={
                        <EnteSwitch
                            checked={settings.includeHidden}
                            onChange={() =>
                                void update({
                                    includeHidden: !settings.includeHidden,
                                })
                            }
                            slotProps={{
                                input: { "aria-label": "Include hidden items" },
                            }}
                        />
                    }
                />
                <RowButtonGroupHint>
                    Also send keys of photos in Hidden. Off by default.
                </RowButtonGroupHint>
            </Stack>
            {scoreRow(
                "Suggest albums when the worker is at least",
                "minAlbumScore",
            )}
            {scoreRow("Suggest cleanup when at least", "minCleanupScore")}
            {scoreRow("Suggest hiding when at least", "minPIIScore")}
        </Stack>
    );
};

const SamplesSection: React.FC<{ state: HomeAIState }> = ({ state }) => {
    const load = useWrapAsyncOperation(loadHomeAISamples);
    const clear = useWrapAsyncOperation(clearHomeAISamples);
    return (
        <Stack sx={{ gap: 1 }}>
            <Typography variant="small" sx={{ color: "text.muted", px: 1 }}>
                Preview the screens before the worker runs. Sample results are
                made up on this device and never uploaded.
            </Typography>
            {state.hasSamples ? (
                <FocusVisibleButton
                    color="secondary"
                    onClick={() => void clear()}
                >
                    Remove sample results
                </FocusVisibleButton>
            ) : (
                <FocusVisibleButton
                    color="secondary"
                    onClick={() => void load()}
                >
                    Load sample results
                </FocusVisibleButton>
            )}
        </Stack>
    );
};
