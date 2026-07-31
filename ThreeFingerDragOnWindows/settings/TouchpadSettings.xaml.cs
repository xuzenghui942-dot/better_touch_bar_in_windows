using System;
using System.Linq;
using ThreeFingerDragOnWindows.utils;

namespace ThreeFingerDragOnWindows.settings;

public sealed partial class TouchpadSettings {

    public TouchpadSettings(){
        InitializeComponent();
        if(App.Instance.HandlerWindow == null || !App.Instance.HandlerWindow.TouchpadInitialized){
            Loader.Visibility = Microsoft.UI.Xaml.Visibility.Visible;
            TouchpadStatus.Visibility = Microsoft.UI.Xaml.Visibility.Collapsed;
            ContactsDebug.Visibility = Microsoft.UI.Xaml.Visibility.Collapsed;
        } else{
            OnTouchpadInitialized();
        }
    }

    public void UpdateContactsText(string text){
        ContactsDebug.Title = "实时输入：\n" + text;
    }

    public void OnTouchpadInitialized(){
        Loader.Visibility = Microsoft.UI.Xaml.Visibility.Collapsed;
        TouchpadStatus.Visibility = Microsoft.UI.Xaml.Visibility.Visible;
        
        if(App.Instance.HandlerWindow.TouchpadExists){
            if(App.Instance.HandlerWindow.InputReceiverInstalled) {
                string deviceInfosString = String.Join("\n", TouchpadHelper.GetAllDeivceInfos().Select(deviceInfo => deviceInfo.ToString()));
                TouchpadStatus.Title = "已检测并成功连接触摸板。\n" + deviceInfosString;
                TouchpadStatus.Severity = Microsoft.UI.Xaml.Controls.InfoBarSeverity.Success;
                ContactsDebug.Visibility = Microsoft.UI.Xaml.Visibility.Visible;
            } else{
                TouchpadStatus.Title = "已检测到触摸板，但无法注册输入接收器。";
                TouchpadStatus.Severity = Microsoft.UI.Xaml.Controls.InfoBarSeverity.Warning;
                ContactsDebug.Visibility = Microsoft.UI.Xaml.Visibility.Collapsed;
            } 
        } else{
            TouchpadStatus.Title = "未检测到触摸板。请确认设备配有 Windows 精确式触摸板。";
            TouchpadStatus.Severity = Microsoft.UI.Xaml.Controls.InfoBarSeverity.Error;
            ContactsDebug.Visibility = Microsoft.UI.Xaml.Visibility.Collapsed;
        }
    }

}
