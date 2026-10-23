// fork: display names for what the home AI worker reports. English only for
// now; the fork does not add strings to upstream's translation files.

/** The search bar's label for a worker entity kind. */
export const homeAIKindLabel = (kind: string) => {
    switch (kind) {
        case "repo":
            return "Open source project";
        case "social":
            return "Social media";
        case "url":
            return "Link";
        case "movie":
        case "show":
            return "Movie or show";
        case "anime":
        case "manga":
            return "Anime or manga";
        case "person":
            return "Person";
        case "device":
            return "Device";
        case "text":
            return "Text in photo";
        default:
            return "Home AI";
    }
};

/** A short label for a cleanup reason or a PII kind. */
export const homeAIReasonLabel = (reason: string) => {
    switch (reason) {
        case "duplicate":
            return "Duplicate";
        case "blur":
            return "Blurry";
        case "accidental":
            return "Accidental shot";
        case "card_number":
            return "Card number";
        case "iban":
            return "Bank account";
        case "passport":
            return "Passport";
        case "national_id":
            return "ID number";
        case "id_document":
            return "ID document";
        case "credential":
            return "Password or key";
        default:
            return reason.replaceAll("_", " ");
    }
};
