import type { Translations } from "./context";

export const en: Translations = {
  // Tab Bar
  tabBar: {
    sync: "Sync",
    devices: "Discover",
    history: "History",
    logs: "Logs",
  },

  // Sidebar (kept for compatibility)
  sidebar: {
    dashboard: "Dashboard",
    pairing: "Pair Device",
    logs: "Logs",
    settings: "Settings",
    appName: "LanBridge",
  },

  // App
  app: {
    dismiss: "Dismiss",
    selectTaskHint: "Select a sync task to view details",
  },

  // Dashboard
  dashboard: {
    title: "Dashboard",
    refresh: "Refresh",
    newTask: "+ New Task",
    thisDevice: "This Device",
    noTasks: "No sync tasks yet",
    noTasksDesc: "Pair a device and create a sync task to get started.",
    createFirst: "Create First Task",
    openFolder: "Open Folder",
    deleteTask: "Delete Task",
    confirmDelete: "Are you sure you want to delete this sync task? The task configuration will be removed, but local files will NOT be deleted.\n\nTask:",
    local: "Local:",
    remote: "Remote:",
    pending: "Pending",
    conflicts: "Conflicts",
    lastUpdated: "Last Updated",
    synced: "synced",
    failed: "failed",
    syncing: "Syncing...",
    syncNow: "Sync Now",
    returnToPrimary: "Return to Primary",
    details: "Details",
    never: "Never",
    incomingInvites: "Incoming Sync Invites",
    inviteFrom: "From",
    invitePathPlaceholder: "Choose or type this computer's receiving folder",
    invitePathRequired: "Enter this computer's receiving folder first.",
    chooseFolder: "Choose Folder",
    acceptInvite: "Accept",
    rejectInvite: "Reject",
  },

  // Pairing
  pairing: {
    title: "Pair Device & Create Sync Task",
    step1Title: "Step 1: Connect to Peer",
    step1Desc:
      "Select a discovered device below, or enter an IP address manually.",
    peerIp: "Peer IP Address",
    port: "Port",
    peerName: "Peer Display Name",
    peerNamePlaceholder: "e.g., MacBook Pro",
    connecting: "Connecting...",
    connect: "Connect",
    refreshDevices: "Refresh Devices",
    refreshingDevices: "Refreshing...",
    checkNetwork: "Check Network",
    checkingNetwork: "Checking...",
    networkOk: "Network check passed",
    networkNeedsAttention: "Network needs attention",
    discoveryRunning: "Discovery is running",
    discoveryStopped: "Discovery is not running",
    discoverySummary: "Listening on {addr}:{port}",
    noDevices: "No devices found yet",
    noDevicesDesc: "Make sure the peer device has this app running, or connect manually.",
    manualFallback: "Enter IP Manually",
    manualFallbackToggle: "Hide Manual Input",
    manualNotice:
      "Manual connection is a fallback when discovery fails. Device identity is still verified before the invite is sent.",
    online: "Online",
    deviceIdShort: "Device ID: {id}",
    addressCandidates: "{count} addresses tried automatically",
    step2Title: "Step 2: Create Sync Task",
    step2Desc:
      "Connected to peer. Choose this computer's folder; the peer will receive an invite and choose its own folder.",
    selectedPeer: "Connected device",
    taskName: "Task Name",
    taskNamePlaceholder: "e.g., Documents Sync",
    localPath: "Local Folder Path",
    localPathPlaceholder: "Choose or type this computer's folder path",
    chooseFolder: "Choose Folder",
    remotePath: "Peer Receiving Folder",
    localRole: "Sync Direction",
    syncMode: "Sync Mode",
    backupMode: "One-way backup: this computer -> peer",
    pullMode: "One-way pull: peer -> this computer",
    twoWayMode: "Two-way sync (coming soon)",
    primary: "One-way backup: this computer -> peer",
    secondary: "One-way pull: peer -> this computer",
    thisDeviceIp: "This device:",
    safetyTitle: "Data Safety:",
    safetyDesc:
      "Deletes are moved to sync history first. The peer folder is allocated by the peer app to avoid typing the other computer's path.",
    createTask: "Trust & Send Invite",
    waitingInviteTitle: "Waiting for peer approval",
    waitingInviteDesc:
      "The peer needs to choose a local folder and accept the invite. The task will be created automatically after approval.",
    invitePending: "Invite sent. Waiting for the peer to respond...",
    inviteSent: "Invite Sent",
    waitingInviteHint: "Open the app on the other computer and handle the incoming invite at the top of the dashboard.",
    inviteRejected: "The peer rejected the invite or it expired.",
  },

  // Task Detail
  task: {
    back: "Back",
    close: "Close",
    loading: "Loading...",
    localPath: "Local Path",
    remotePath: "Remote Path",
    status: "Status",
    active: "Active",
    paused: "Paused",
    peerStatus: "Peer Status",
    peerConnected: "Connected",
    peerDisconnected: "Disconnected",
    peerChecking: "Checking...",
    syncBlockedOffline: "The peer is disconnected. Primary sync will resume automatically after reconnection.",
    created: "Created",
    scanAndSync: "Scan & Sync",
    syncing: "Syncing...",
    scanOnly: "Scan Only",
    pause: "Pause",
    resume: "Resume",
    pendingReturn: "Pending Return",
    conflicts: "Conflicts",
    noSafeReturnItems: "No files are safe to return yet. Resolve conflicts or reconnect the primary first.",
    viewHistory: "View History →",
    lastResults: "Last Sync Results",
    files: "File Status",
    subTabs: {
      info: "Info",
      returnSync: "Return Sync",
      history: "History",
    },
  },

  // Return Sync
  returnSync: {
    title: "Pending Return-Sync",
    back: "Back",
    conflictsBanner: "conflict(s) detected",
    conflictsDesc:
      "Pending return files differ from the primary. Choose keep both or overwrite primary first.",
    noPending: "No pending changes",
    noPendingDesc: "Secondary-side files will appear here when created or modified.",
    selectSafe: "Select Safe Items",
    selected: "selected",
    syncing: "Syncing...",
    returnSyncN: "Return-Sync",
    file: "File(s)",
    syncOne: "Sync One",
    resolve: "Resolve",
    results: "Return-Sync Results",
    primaryCheckFailed: "Primary status could not be checked, so safe return-sync is unavailable.",
  },

  // Conflict Modal
  conflict: {
    title: "Both sides changed this file",
    description: "Choose how to handle it before anything is overwritten:",
    hasConflict: "",
    hashWarning:
      "Full hash verification is unavailable for this large file. Size and modification time are used as a fallback.",
    primarySide: "Primary current version",
    secondarySide: "Secondary pending version",
    modified: "Modified:",
    note: "No silent overwrite:",
    noteDesc:
      "The primary file is backed up before overwrite.",
    cancel: "Cancel",
    keepBoth: "Keep Both",
    overwrite: "Use Secondary (backup first)",
  },

  // History
  history: {
    title: "Sync History",
    back: "Back",
    allTasks: "All Tasks",
    cleanup: "Clean History Files",
    loading: "Loading history...",
    noEntries: "No history entries",
    noEntriesDesc:
      "Files deleted from primary or overwritten during conflict resolution will appear here.",
    restore: "Restore",
    restoring: "Restoring...",
    trash: "Trash",
    overwritten: "Overwritten",
  },

  // Logs
  logs: {
    title: "Sync Logs",
    back: "Back",
    refresh: "Refresh",
    loading: "Loading logs...",
    noLogs: "No log entries",
    noLogsDesc: "Sync events will be recorded here.",
  },

  // Roles
  role: {
    primary: "Primary",
    secondary: "Secondary",
  },

  // Settings
  settings: {
    title: "Settings",
    back: "Back",
    fileStatus: "File Status",
    language: "Language",
    langZh: "中文",
    langEn: "English",
    historyRetention: "History Retention",
    retentionPeriod: "Retention Period",
    days: "days",
    sizeLimit: "Size Limit",
    mb: "MB",
    about: "About",
    version: "Version",
    syncModel: "Sync Model",
    syncModelDesc: "One-way backup / one-way pull (delete protection)",
    hashAlgorithm: "Hash Algorithm",
  },
};
