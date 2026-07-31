using System;

namespace ThreeFingerDragOnWindows.utils;

public static class UiAutomationCrashGuardPolicy{
    private const uint WmGetObject = 0x003D;
    private const int UiaRootObjectId = -25;

    public static bool ShouldSuppress(uint message, IntPtr objectId){
        if(message != WmGetObject) return false;

        // LPARAM can arrive either sign-extended or zero-extended on a 64-bit process.
        return unchecked((int) objectId.ToInt64()) == UiaRootObjectId;
    }
}
