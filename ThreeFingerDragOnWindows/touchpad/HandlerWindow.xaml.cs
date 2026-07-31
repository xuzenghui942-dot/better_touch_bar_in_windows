using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using System.Threading;
using System.Timers;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Input;
using ThreeFingerDragEngine.utils;
using ThreeFingerDragOnWindows.threefingerdrag;
using ThreeFingerDragOnWindows.utils;

namespace ThreeFingerDragOnWindows.touchpad;

public sealed partial class HandlerWindow : Window {
    private readonly App _app;
    private readonly ContactsManager _contactsManager;
    private readonly ThreeFingerDrag _threeFingersDrag;

    public volatile bool TouchpadInitialized; // Became true when the touchpad check is done, but does not confirm that the touchpad has been registered
    public volatile bool TouchpadExists;
    public volatile bool InputReceiverInstalled;
    private int _uiContactUpdatePending;

    public HandlerWindow(App app){
        Logger.Log("Starting HandlerWindow...");
        InitializeComponent();

        _app = app;
        _contactsManager = new ContactsManager(this);
        _threeFingersDrag = new ThreeFingerDrag();

        // Let the _handlerWindow to be defined in App.xaml.cs before initializing the source
        Utils.runOnMainThreadAfter(100, () => {
            _contactsManager.InitializeSource();
        });
        Closed += (_, _) => _contactsManager.Dispose();

    }

    // TaskbarIcon Actions
    private void OpenSettingsWindow(object sender, ExecuteRequestedEventArgs e){
        Logger.Log("Opening SettingsWindow from HandlerWindow TaskbarIcon");
        _app.OpenSettingsWindow();
    }

    private void QuitApp(object sender, ExecuteRequestedEventArgs e){
        Logger.Log("Quitting App from HandlerWindow TaskbarIcon");
        _app.Quit();
    }


    // Touchpad
    // Called when the touchpad is detected and the events handlers are registered (or not)
    public void OnTouchpadInitialized(bool touchpadExists, bool inputReceiverInstalled){
        TouchpadExists = touchpadExists;
        InputReceiverInstalled = inputReceiverInstalled;
        if(!touchpadExists) Logger.Log("Touchpad is not detected.");
        else if(!inputReceiverInstalled) Logger.Log("Touchpad is detected but the input receiver couldn't be installed.");
        else Logger.Log("Touchpad is detected and registered.");

        TouchpadInitialized = true;
        _app.DispatcherQueue.TryEnqueue(_app.OnTouchpadInitialized);
    }

    // Called when a new set of contacts has been registered

    private TouchpadContact[] _oldContacts = Array.Empty<TouchpadContact>();
    private long _lastContactCtms = Ctms();

    public void OnTouchpadContact(IntPtr currentDevice, List<TouchpadContact> contacts){
        if(App.SettingsData.ThreeFingerDrag){
            _threeFingersDrag.OnTouchpadContact(currentDevice, _oldContacts, contacts.ToArray(), Ctms() - _lastContactCtms);
        }

        QueueContactPreview(currentDevice, contacts);
        _lastContactCtms = Ctms();
        _oldContacts = contacts.ToArray();
    }

    private void QueueContactPreview(IntPtr currentDevice, List<TouchpadContact> contacts){
        if(App.SettingsWindow == null) return;
        if(Interlocked.CompareExchange(ref _uiContactUpdatePending, 1, 0) != 0) return;

        TouchpadContact[] snapshot = contacts.ToArray();
        if(!_app.DispatcherQueue.TryEnqueue(() => {
               try{
                   _app.OnTouchpadContact(currentDevice, snapshot);
               } finally{
                   Volatile.Write(ref _uiContactUpdatePending, 0);
               }
           })){
            Volatile.Write(ref _uiContactUpdatePending, 0);
        }
    }

    private static long Ctms(){
        return new DateTimeOffset(DateTime.UtcNow).ToUnixTimeMilliseconds();
    }
}
