using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using ThreeFingerDragEngine.utils;
using ThreeFingerDragOnWindows.utils;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Threading;

namespace ThreeFingerDragOnWindows.touchpad;

public sealed class ContactsManager : IDisposable{
    private const uint WmClose = 0x0010;
    private const uint WmDestroy = 0x0002;
    private static readonly IntPtr HwndMessage = new(-3);

    private readonly HandlerWindow _source;
    private readonly Thread _inputThread;
    private readonly WindowProc _windowProc;
    private readonly ManualResetEventSlim _windowReady = new(false);
    private IntPtr _hwnd;
    private IntPtr _moduleHandle;
    private string _windowClassName;
    private int _initializationStarted;
    private int _disposed;

    public ContactsManager(HandlerWindow source){
        _source = source;
        _windowProc = WindowProcess;
        _inputThread = new Thread(InputThreadMain){
            IsBackground = true,
            Name = "ThreeFingerDrag Raw Input"
        };
    }

    public void InitializeSource(){
        if(Interlocked.Exchange(ref _initializationStarted, 1) != 0) return;
        _inputThread.Start();
    }

    private void InputThreadMain(){
        try{
            _moduleHandle = GetModuleHandle(null);
            _windowClassName = $"ThreeFingerDrag.RawInput.{Environment.ProcessId}.{GetHashCode():X8}";

            WndClassEx windowClass = new(){
                cbSize = (uint) Marshal.SizeOf<WndClassEx>(),
                lpfnWndProc = _windowProc,
                hInstance = _moduleHandle,
                lpszClassName = _windowClassName
            };

            if(RegisterClassEx(ref windowClass) == 0){
                throw new Win32Exception(Marshal.GetLastWin32Error(), "无法注册触摸板输入窗口。");
            }

            _hwnd = CreateWindowEx(
                0,
                _windowClassName,
                string.Empty,
                0,
                0,
                0,
                0,
                0,
                HwndMessage,
                IntPtr.Zero,
                _moduleHandle,
                IntPtr.Zero);

            if(_hwnd == IntPtr.Zero){
                throw new Win32Exception(Marshal.GetLastWin32Error(), "无法创建触摸板输入窗口。");
            }

            bool touchpadExists = TouchpadHelper.Exists();
            bool inputReceiverInstalled = TouchpadHelper.RegisterInput(_hwnd);
            Logger.Log($"Raw Input worker started on thread {GetCurrentThreadId()}, hwnd={_hwnd}.");
            _source.OnTouchpadInitialized(touchpadExists, inputReceiverInstalled);
        } catch(Exception exception){
            Logger.Log("Raw Input worker initialization failed: " + exception);
            _source.OnTouchpadInitialized(false, false);
        } finally{
            _windowReady.Set();
        }

        if(_hwnd == IntPtr.Zero) return;

        while(true){
            int result = GetMessage(out Message message, IntPtr.Zero, 0, 0);
            if(result <= 0) break;
            TranslateMessage(ref message);
            DispatchMessage(ref message);
        }

        if(IsWindow(_hwnd)){
            DestroyWindow(_hwnd);
        }
        _hwnd = IntPtr.Zero;

        if(!string.IsNullOrEmpty(_windowClassName)){
            UnregisterClass(_windowClassName, _moduleHandle);
        }
        Logger.Log("Raw Input worker stopped.");
    }

    // WindowProc Listener
    private IntPtr WindowProcess(IntPtr hwnd, uint message, IntPtr wParam, IntPtr lParam){
        try{
            switch(message){
                case TouchpadHelper.WM_INPUT:
                    var (currentDevice, contacts, count) = TouchpadHelper.ParseInput(lParam);
                    ReceiveTouchpadContacts(currentDevice, contacts, count);
                    break;
                case TouchpadHelper.WM_INPUT_DEVICE_CHANGE:
                    _source.OnTouchpadInitialized(TouchpadHelper.Exists(lParam), true);
                    break;
                case WmClose:
                    DestroyWindow(hwnd);
                    return IntPtr.Zero;
                case WmDestroy:
                    PostQuitMessage(0);
                    return IntPtr.Zero;
            }
        } catch(Exception exception){
            Logger.Log("Raw Input message processing failed: " + exception);
        }

        return DefWindowProc(hwnd, message, wParam, lParam);
    }


    // Contacts managements
    private List<TouchpadContact> _lastContacts = new();
    private uint _targetContactCount;

    private void ReceiveTouchpadContacts(IntPtr currentDevice, List<TouchpadContact> contacts, uint count){
        if(contacts == null || contacts.Count == 0){
            Logger.Log("Receiving empty contacts with cC=" + count);
            return;
        }

        // Regular contact list
        if(count == contacts.Count){
            Logger.Log("+ Receiving regular contact list: " +  string.Join(", ", contacts.Select(c => c.ToString())));
            _source.OnTouchpadContact(currentDevice, contacts);
            _lastContacts.Clear();
            return;
        }

        // Partial contact list (always sent after an incomplete contact list)
        if(count == 0){
            Logger.Log("Receiving partial contact list: " + string.Join(", ", contacts.Select(c => c.ToString())));
            _lastContacts.AddRange(contacts);
            _lastContacts = RemoveDuplicates(_lastContacts);

            if(_targetContactCount == 0){
                Logger.Log("[WARNING] Target contact count not received yet through an invalid contact list.");
                return;
            }

            if(_lastContacts.Count > _targetContactCount){
                Logger.Log("[WARNING] LastContact list has more contacts than expected: " + string.Join(", ", _lastContacts.Select(c => c.ToString())));
                _lastContacts = _lastContacts.Take((int) _targetContactCount).ToList();
                _source.OnTouchpadContact(currentDevice, _lastContacts);
                _lastContacts.Clear();

            }
            if(_lastContacts.Count == _targetContactCount){
                Logger.Log("+ LastContact list has correct length: " + string.Join(", ", _lastContacts.Select(c => c.ToString())));
                _source.OnTouchpadContact(currentDevice, _lastContacts);
                _lastContacts.Clear();
            }
            return;
        }

        // Old partial contact list has not been submitted yet : duplicating
        if(_lastContacts.Count != 0){
            Logger.Log("[WARNING] New incomplete contact list received while old lastContacts not empty: " + contacts.Count);

            if(_lastContacts.Count < _targetContactCount){
                var lastContact = _lastContacts.Last();
                var maxId = _lastContacts.Max(c => c.ContactId);
                for(int i = 1; i <= _targetContactCount - _lastContacts.Count; i++){
                    _lastContacts.Add(new TouchpadContact(maxId + i, lastContact.X, lastContact.Y));
                }
                Logger.Log("+ Duplicated last contact to fulfil list: " + string.Join(", ", _lastContacts.Select(c => c.ToString())));
            }else if(_lastContacts.Count > _targetContactCount){
                Logger.Log("[WARNING] LastContact list has more contacts than expected: " + string.Join(", ", _lastContacts.Select(c => c.ToString())));
                _lastContacts = _lastContacts.Take((int) _targetContactCount).ToList();
            }

            Logger.Log("+ LastContact list has correct length: " + string.Join(", ", _lastContacts.Select(c => c.ToString())));

            _source.OnTouchpadContact(currentDevice, _lastContacts);
            _lastContacts.Clear();
        }

        // Regular contact list with more contacts than expected (unlikely to happen)
        if(count <= contacts.Count){
            Logger.Log("[WARNING] Received contact list with more contacts than expected: " + string.Join(", ", contacts.Select(c => c.ToString())));
            contacts = contacts.Take((int) count).ToList();
            Logger.Log("+ Contact list has been clamped: " + string.Join(", ", contacts.Select(c => c.ToString())));
            _source.OnTouchpadContact(currentDevice, contacts);
            _lastContacts.Clear();
            return;
        }

        // Here, 0 < contacts.Length < count and lastContacts is empty: incomplete contact list
        _targetContactCount = count;
        _lastContacts = contacts;
        Logger.Log("Receiving incomplete contact count, waiting for partial contacts: " + string.Join(", ", contacts.Select(c => c.ToString())));
    }

    private List<TouchpadContact> RemoveDuplicates(List<TouchpadContact> contacts){
        var uniqueContacts = new List<TouchpadContact>();
        foreach(var contact in contacts){
            if(!uniqueContacts.Any(c => c.ContactId == contact.ContactId)){
                uniqueContacts.Add(contact);
            }
        }
        if(uniqueContacts.Count != contacts.Count){
            Logger.Log("[WARNING] Duplicate contacts ID in list. Removing duplicates: " + string.Join(", ", uniqueContacts.Select(c => c.ToString())));
        }
        return uniqueContacts;
    }

    public void Dispose(){
        if(Interlocked.Exchange(ref _disposed, 1) != 0) return;
        if(Volatile.Read(ref _initializationStarted) == 0) return;

        _windowReady.Wait(TimeSpan.FromSeconds(2));
        IntPtr windowHandle = _hwnd;
        if(windowHandle != IntPtr.Zero){
            PostMessage(windowHandle, WmClose, IntPtr.Zero, IntPtr.Zero);
        }
        _inputThread.Join(TimeSpan.FromSeconds(2));
        _windowReady.Dispose();
    }

    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate IntPtr WindowProc(IntPtr hwnd, uint message, IntPtr wParam, IntPtr lParam);

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct WndClassEx{
        public uint cbSize;
        public uint style;
        public WindowProc lpfnWndProc;
        public int cbClsExtra;
        public int cbWndExtra;
        public IntPtr hInstance;
        public IntPtr hIcon;
        public IntPtr hCursor;
        public IntPtr hbrBackground;
        [MarshalAs(UnmanagedType.LPWStr)]
        public string lpszMenuName;
        [MarshalAs(UnmanagedType.LPWStr)]
        public string lpszClassName;
        public IntPtr hIconSm;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct Message{
        public IntPtr hwnd;
        public uint message;
        public IntPtr wParam;
        public IntPtr lParam;
        public uint time;
        public Point point;
        public uint privateValue;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct Point{
        public int x;
        public int y;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    private static extern IntPtr GetModuleHandle(string moduleName);

    [DllImport("kernel32.dll")]
    private static extern uint GetCurrentThreadId();

    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern ushort RegisterClassEx(ref WndClassEx windowClass);

    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool UnregisterClass(string className, IntPtr instance);

    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr CreateWindowEx(
        uint extendedStyle,
        string className,
        string windowName,
        uint style,
        int x,
        int y,
        int width,
        int height,
        IntPtr parent,
        IntPtr menu,
        IntPtr instance,
        IntPtr parameter);

    [DllImport("user32.dll")]
    private static extern IntPtr DefWindowProc(
        IntPtr hwnd,
        uint message,
        IntPtr wParam,
        IntPtr lParam);

    [DllImport("user32.dll")]
    private static extern int GetMessage(
        out Message message,
        IntPtr hwnd,
        uint minimumMessage,
        uint maximumMessage);

    [DllImport("user32.dll")]
    private static extern bool TranslateMessage(ref Message message);

    [DllImport("user32.dll")]
    private static extern IntPtr DispatchMessage(ref Message message);

    [DllImport("user32.dll")]
    private static extern bool DestroyWindow(IntPtr hwnd);

    [DllImport("user32.dll")]
    private static extern bool IsWindow(IntPtr hwnd);

    [DllImport("user32.dll")]
    private static extern void PostQuitMessage(int exitCode);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool PostMessage(
        IntPtr hwnd,
        uint message,
        IntPtr wParam,
        IntPtr lParam);
}
