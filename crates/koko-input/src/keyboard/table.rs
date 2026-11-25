pub(super) const LEFT_CONTROL: u16 = 0x3b; // kVK_Control
pub(super) const LEFT_COMMAND: u16 = 0x37; // kVK_Command
pub(super) const RIGHT_CONTROL: u16 = 0x3e; // kVK_RightControl
pub(super) const RIGHT_COMMAND: u16 = 0x36; // kVK_RightCommand

// Mac position, Microsoft Set 1 make (including E0), and reference Windows VK.
// Each row names its Events.h constant and Keyboard/Keypad-page HID usage
// in Microsoft's Scan 1 Make table. Source links are in docs/windows-plan.md.
pub(super) const KEYS: &[(u16, u16, Option<u16>)] = &[
    // ANSI letters and digits
    (0x00, 0x001e, Some(0x41)), // kVK_ANSI_A, HID 04
    (0x0b, 0x0030, Some(0x42)), // kVK_ANSI_B, HID 05
    (0x08, 0x002e, Some(0x43)), // kVK_ANSI_C, HID 06
    (0x02, 0x0020, Some(0x44)), // kVK_ANSI_D, HID 07
    (0x0e, 0x0012, Some(0x45)), // kVK_ANSI_E, HID 08
    (0x03, 0x0021, Some(0x46)), // kVK_ANSI_F, HID 09
    (0x05, 0x0022, Some(0x47)), // kVK_ANSI_G, HID 0A
    (0x04, 0x0023, Some(0x48)), // kVK_ANSI_H, HID 0B
    (0x22, 0x0017, Some(0x49)), // kVK_ANSI_I, HID 0C
    (0x26, 0x0024, Some(0x4a)), // kVK_ANSI_J, HID 0D
    (0x28, 0x0025, Some(0x4b)), // kVK_ANSI_K, HID 0E
    (0x25, 0x0026, Some(0x4c)), // kVK_ANSI_L, HID 0F
    (0x2e, 0x0032, Some(0x4d)), // kVK_ANSI_M, HID 10
    (0x2d, 0x0031, Some(0x4e)), // kVK_ANSI_N, HID 11
    (0x1f, 0x0018, Some(0x4f)), // kVK_ANSI_O, HID 12
    (0x23, 0x0019, Some(0x50)), // kVK_ANSI_P, HID 13
    (0x0c, 0x0010, Some(0x51)), // kVK_ANSI_Q, HID 14
    (0x0f, 0x0013, Some(0x52)), // kVK_ANSI_R, HID 15
    (0x01, 0x001f, Some(0x53)), // kVK_ANSI_S, HID 16
    (0x11, 0x0014, Some(0x54)), // kVK_ANSI_T, HID 17
    (0x20, 0x0016, Some(0x55)), // kVK_ANSI_U, HID 18
    (0x09, 0x002f, Some(0x56)), // kVK_ANSI_V, HID 19
    (0x0d, 0x0011, Some(0x57)), // kVK_ANSI_W, HID 1A
    (0x07, 0x002d, Some(0x58)), // kVK_ANSI_X, HID 1B
    (0x10, 0x0015, Some(0x59)), // kVK_ANSI_Y, HID 1C
    (0x06, 0x002c, Some(0x5a)), // kVK_ANSI_Z, HID 1D
    (0x12, 0x0002, Some(0x31)), // kVK_ANSI_1, HID 1E
    (0x13, 0x0003, Some(0x32)), // kVK_ANSI_2, HID 1F
    (0x14, 0x0004, Some(0x33)), // kVK_ANSI_3, HID 20
    (0x15, 0x0005, Some(0x34)), // kVK_ANSI_4, HID 21
    (0x17, 0x0006, Some(0x35)), // kVK_ANSI_5, HID 22
    (0x16, 0x0007, Some(0x36)), // kVK_ANSI_6, HID 23
    (0x1a, 0x0008, Some(0x37)), // kVK_ANSI_7, HID 24
    (0x1c, 0x0009, Some(0x38)), // kVK_ANSI_8, HID 25
    (0x19, 0x000a, Some(0x39)), // kVK_ANSI_9, HID 26
    (0x1d, 0x000b, Some(0x30)), // kVK_ANSI_0, HID 27
    // ANSI punctuation (reference VKs from the US layout)
    (0x1b, 0x000c, Some(0xbd)), // kVK_ANSI_Minus, HID 2D
    (0x18, 0x000d, Some(0xbb)), // kVK_ANSI_Equal, HID 2E
    (0x21, 0x001a, Some(0xdb)), // kVK_ANSI_LeftBracket, HID 2F
    (0x1e, 0x001b, Some(0xdd)), // kVK_ANSI_RightBracket, HID 30
    (0x2a, 0x002b, Some(0xdc)), // kVK_ANSI_Backslash, HID 31
    (0x29, 0x0027, Some(0xba)), // kVK_ANSI_Semicolon, HID 33
    (0x27, 0x0028, Some(0xde)), // kVK_ANSI_Quote, HID 34
    (0x32, 0x0029, Some(0xc0)), // kVK_ANSI_Grave, HID 35
    (0x2b, 0x0033, Some(0xbc)), // kVK_ANSI_Comma, HID 36
    (0x2f, 0x0034, Some(0xbe)), // kVK_ANSI_Period, HID 37
    (0x2c, 0x0035, Some(0xbf)), // kVK_ANSI_Slash, HID 38
    // Editing and navigation
    (0x24, 0x001c, Some(0x0d)), // kVK_Return, HID 28
    (0x35, 0x0001, Some(0x1b)), // kVK_Escape, HID 29
    (0x33, 0x000e, Some(0x08)), // kVK_Delete, HID 2A
    (0x30, 0x000f, Some(0x09)), // kVK_Tab, HID 2B
    (0x31, 0x0039, Some(0x20)), // kVK_Space, HID 2C
    (0x39, 0x003a, Some(0x14)), // kVK_CapsLock, HID 39
    (0x72, 0xe052, Some(0x2d)), // kVK_Help, HID 49
    (0x73, 0xe047, Some(0x24)), // kVK_Home, HID 4A
    (0x74, 0xe049, Some(0x21)), // kVK_PageUp, HID 4B
    (0x75, 0xe053, Some(0x2e)), // kVK_ForwardDelete, HID 4C
    (0x77, 0xe04f, Some(0x23)), // kVK_End, HID 4D
    (0x79, 0xe051, Some(0x22)), // kVK_PageDown, HID 4E
    (0x7c, 0xe04d, Some(0x27)), // kVK_RightArrow, HID 4F
    (0x7b, 0xe04b, Some(0x25)), // kVK_LeftArrow, HID 50
    (0x7d, 0xe050, Some(0x28)), // kVK_DownArrow, HID 51
    (0x7e, 0xe048, Some(0x26)), // kVK_UpArrow, HID 52
    // Function keys
    (0x7a, 0x003b, Some(0x70)), // kVK_F1, HID 3A
    (0x78, 0x003c, Some(0x71)), // kVK_F2, HID 3B
    (0x63, 0x003d, Some(0x72)), // kVK_F3, HID 3C
    (0x76, 0x003e, Some(0x73)), // kVK_F4, HID 3D
    (0x60, 0x003f, Some(0x74)), // kVK_F5, HID 3E
    (0x61, 0x0040, Some(0x75)), // kVK_F6, HID 3F
    (0x62, 0x0041, Some(0x76)), // kVK_F7, HID 40
    (0x64, 0x0042, Some(0x77)), // kVK_F8, HID 41
    (0x65, 0x0043, Some(0x78)), // kVK_F9, HID 42
    (0x6d, 0x0044, Some(0x79)), // kVK_F10, HID 43
    (0x67, 0x0057, Some(0x7a)), // kVK_F11, HID 44
    (0x6f, 0x0058, Some(0x7b)), // kVK_F12, HID 45
    (0x69, 0x0064, Some(0x7c)), // kVK_F13, HID 68
    (0x6b, 0x0065, Some(0x7d)), // kVK_F14, HID 69
    (0x71, 0x0066, Some(0x7e)), // kVK_F15, HID 6A
    (0x6a, 0x0067, Some(0x7f)), // kVK_F16, HID 6B
    (0x40, 0x0068, Some(0x80)), // kVK_F17, HID 6C
    (0x4f, 0x0069, Some(0x81)), // kVK_F18, HID 6D
    (0x50, 0x006a, Some(0x82)), // kVK_F19, HID 6E
    (0x5a, 0x006b, Some(0x83)), // kVK_F20, HID 6F
    // Keypad (digit VKs with Num Lock on)
    (0x53, 0x004f, Some(0x61)), // kVK_ANSI_Keypad1, HID 59
    (0x54, 0x0050, Some(0x62)), // kVK_ANSI_Keypad2, HID 5A
    (0x55, 0x0051, Some(0x63)), // kVK_ANSI_Keypad3, HID 5B
    (0x56, 0x004b, Some(0x64)), // kVK_ANSI_Keypad4, HID 5C
    (0x57, 0x004c, Some(0x65)), // kVK_ANSI_Keypad5, HID 5D
    (0x58, 0x004d, Some(0x66)), // kVK_ANSI_Keypad6, HID 5E
    (0x59, 0x0047, Some(0x67)), // kVK_ANSI_Keypad7, HID 5F
    (0x5b, 0x0048, Some(0x68)), // kVK_ANSI_Keypad8, HID 60
    (0x5c, 0x0049, Some(0x69)), // kVK_ANSI_Keypad9, HID 61
    (0x52, 0x0052, Some(0x60)), // kVK_ANSI_Keypad0, HID 62
    (0x41, 0x0053, Some(0x6e)), // kVK_ANSI_KeypadDecimal, HID 63
    (0x43, 0x0037, Some(0x6a)), // kVK_ANSI_KeypadMultiply, HID 55
    (0x45, 0x004e, Some(0x6b)), // kVK_ANSI_KeypadPlus, HID 57
    (0x4b, 0xe035, Some(0x6f)), // kVK_ANSI_KeypadDivide, HID 54
    (0x4c, 0xe01c, Some(0x0d)), // kVK_ANSI_KeypadEnter, HID 58
    (0x4e, 0x004a, Some(0x6d)), // kVK_ANSI_KeypadMinus, HID 56
    (0x51, 0x0059, Some(0x92)), // kVK_ANSI_KeypadEquals, HID 67
    (0x47, 0x0045, Some(0x90)), // kVK_ANSI_KeypadClear, HID 53
    // Side-specific modifiers
    (LEFT_CONTROL, 0x001d, Some(0xa2)),  // kVK_Control, HID E0
    (0x38, 0x002a, Some(0xa0)),          // kVK_Shift, HID E1
    (0x3a, 0x0038, Some(0xa4)),          // kVK_Option, HID E2
    (LEFT_COMMAND, 0xe05b, Some(0x5b)),  // kVK_Command, HID E3
    (RIGHT_CONTROL, 0xe01d, Some(0xa3)), // kVK_RightControl, HID E4
    (0x3c, 0x0036, Some(0xa1)),          // kVK_RightShift, HID E5
    (0x3d, 0xe038, Some(0xa5)),          // kVK_RightOption, HID E6
    (RIGHT_COMMAND, 0xe05c, Some(0x5c)), // kVK_RightCommand, HID E7
    // ISO/JIS and Application (Apple IOHIDFamily HID bridge; no fixed VK for keypad comma)
    (0x0a, 0x0056, Some(0xe2)), // kVK_ISO_Section, HID 64
    (0x5e, 0x0073, Some(0xe2)), // kVK_JIS_Underscore, HID 87
    (0x5d, 0x007d, Some(0xdc)), // kVK_JIS_Yen, HID 89
    (0x5f, 0x007e, None),       // kVK_JIS_KeypadComma, HID 85
    (0x6e, 0xe05d, Some(0x5d)), // kVK_ContextualMenu, HID 65
];
