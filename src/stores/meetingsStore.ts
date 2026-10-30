import { create } from "zustand";
import {
  commands,
  events,
  type MeetingListItem,
  type RecordingInfo,
} from "@/bindings";

// The Meetings list and the meeting being recorded, kept current by the
// backend's meeting events.
interface MeetingsStore {
  meetings: MeetingListItem[];
  loaded: boolean;
  loadFailed: boolean;
  query: string;
  recording: RecordingInfo | null;
  initialized: boolean;
  initialize: () => Promise<void>;
  setQuery: (query: string) => void;
  reload: () => Promise<void>;
}

let reloadSerial = 0;

export const useMeetingsStore = create<MeetingsStore>()((set, get) => ({
  meetings: [],
  loaded: false,
  loadFailed: false,
  query: "",
  recording: null,
  initialized: false,

  initialize: async () => {
    if (get().initialized) return;
    set({ initialized: true });

    await events.meetingRecordingEvent.listen(({ payload }) => {
      set({ recording: payload.recording });
    });
    await events.meetingChangedEvent.listen(({ payload }) => {
      const { id, meeting } = payload;
      // A search's matches can change with any edit: ask again.
      if (get().query.trim()) {
        void get().reload();
        return;
      }
      set((state) => {
        const rest = state.meetings.filter((m) => m.id !== id);
        if (!meeting) return { meetings: rest };
        const meetings = [...rest, meeting].sort(
          (a, b) => b.started_at - a.started_at,
        );
        return { meetings };
      });
    });

    set({ recording: await commands.getMeetingRecording() });
    await get().reload();
  },

  setQuery: (query) => {
    set({ query });
    void get().reload();
  },

  reload: async () => {
    const serial = ++reloadSerial;
    const query = get().query.trim();
    const result = await commands.listMeetings(query || null);
    // A newer search already answered.
    if (serial !== reloadSerial) return;
    if (result.status === "ok") {
      set({ meetings: result.data, loaded: true, loadFailed: false });
    } else {
      console.error("Failed to load meetings:", result.error);
      set({ loaded: true, loadFailed: true });
    }
  },
}));
