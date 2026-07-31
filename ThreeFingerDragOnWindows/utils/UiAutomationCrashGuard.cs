using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Windows.Automation.Provider;
using Microsoft.UI.Xaml;
using WinRT.Interop;

namespace ThreeFingerDragOnWindows.utils;

/// <summary>
/// Keeps cross-process UI Automation clients out of the affected WinUI
/// provider tree by returning a minimal, thread-safe root provider.
/// Remove this guard after microsoft-ui-xaml#11139 is fixed in the runtime.
/// </summary>
public sealed class UiAutomationCrashGuard{
    private const int GwlWndProc = -4;
    private const uint WmCreate = 0x0001;
    private const uint WmParentNotify = 0x0210;

    private readonly object _syncRoot = new();
    private readonly Dictionary<IntPtr, IntPtr> _originalWindowProcedures = new();
    private readonly IRawElementProviderSimple _safeRootProvider = new SafeRootProvider();
    private readonly IntPtr _windowHandle;
    private readonly WindowProc _windowProc;

    public UiAutomationCrashGuard(Window window){
        _windowHandle = WindowNative.GetWindowHandle(window);
        _windowProc = GuardedWindowProc;

        HookWindowTree();
        window.Activated += (_, _) => HookWindowTree();
    }

    private void HookWindowTree(){
        TryHookWindow(_windowHandle, true);
        EnumChildWindows(_windowHandle, (childWindow, _) => {
            TryHookWindow(childWindow, false);
            return true;
        }, IntPtr.Zero);
    }

    private void TryHookWindow(IntPtr windowHandle, bool throwOnFailure){
        lock(_syncRoot){
            if(_originalWindowProcedures.ContainsKey(windowHandle)) return;
        }

        Marshal.SetLastPInvokeError(0);
        IntPtr originalWindowProcedure = SetWindowProc(
            windowHandle,
            GwlWndProc,
            Marshal.GetFunctionPointerForDelegate(_windowProc));

        int error = Marshal.GetLastPInvokeError();
        if(originalWindowProcedure == IntPtr.Zero){
            int failureError = error == 0 ? 31 : error;
            if(throwOnFailure){
                throw new Win32Exception(failureError, "无法安装 UI Automation 崩溃防护。");
            }

            Logger.Log($"无法为子窗口 {windowHandle} 安装 UI Automation 崩溃防护：{failureError}");
            return;
        }

        lock(_syncRoot){
            _originalWindowProcedures[windowHandle] = originalWindowProcedure;
        }
    }

    private IntPtr GuardedWindowProc(IntPtr hwnd, uint message, IntPtr wParam, IntPtr lParam){
        if(message == WmParentNotify && unchecked((ushort) wParam.ToInt64()) == WmCreate){
            TryHookWindow(lParam, false);
        }

        if(UiAutomationCrashGuardPolicy.ShouldSuppress(message, lParam)){
            try{
                return AutomationInteropProvider.ReturnRawElementProvider(
                    hwnd,
                    wParam,
                    lParam,
                    _safeRootProvider);
            } catch(Exception exception){
                Logger.Log($"返回安全 UI Automation 提供程序时发生异常：{exception}");
                return IntPtr.Zero;
            }
        }

        IntPtr originalWindowProcedure;
        lock(_syncRoot){
            if(!_originalWindowProcedures.TryGetValue(hwnd, out originalWindowProcedure)){
                return DefWindowProc(hwnd, message, wParam, lParam);
            }
        }

        return CallWindowProc(originalWindowProcedure, hwnd, message, wParam, lParam);
    }

    private sealed class SafeRootProvider : IRawElementProviderSimple{
        public ProviderOptions ProviderOptions =>
            ProviderOptions.ServerSideProvider | ProviderOptions.UseComThreading;

        public IRawElementProviderSimple HostRawElementProvider => null;

        public object GetPatternProvider(int patternId){
            return null;
        }

        public object GetPropertyValue(int propertyId){
            return null;
        }
    }

    private delegate IntPtr WindowProc(IntPtr hwnd, uint message, IntPtr wParam, IntPtr lParam);

    private static IntPtr SetWindowProc(IntPtr windowHandle, int index, IntPtr newValue){
        return IntPtr.Size == 8
            ? SetWindowLongPtr64(windowHandle, index, newValue)
            : new IntPtr(SetWindowLong32(windowHandle, index, newValue.ToInt32()));
    }

    [DllImport("user32.dll", EntryPoint = "SetWindowLongW", SetLastError = true)]
    private static extern int SetWindowLong32(
        IntPtr windowHandle,
        int index,
        int newValue);

    [DllImport("user32.dll", EntryPoint = "SetWindowLongPtrW", SetLastError = true)]
    private static extern IntPtr SetWindowLongPtr64(
        IntPtr windowHandle,
        int index,
        IntPtr newValue);

    [DllImport("user32.dll", EntryPoint = "CallWindowProcW")]
    private static extern IntPtr CallWindowProc(
        IntPtr previousWindowProc,
        IntPtr windowHandle,
        uint message,
        IntPtr wParam,
        IntPtr lParam);

    [DllImport("user32.dll", EntryPoint = "DefWindowProcW")]
    private static extern IntPtr DefWindowProc(
        IntPtr windowHandle,
        uint message,
        IntPtr wParam,
        IntPtr lParam);

    private delegate bool EnumChildProc(IntPtr windowHandle, IntPtr parameter);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool EnumChildWindows(
        IntPtr parentWindow,
        EnumChildProc callback,
        IntPtr parameter);

}
