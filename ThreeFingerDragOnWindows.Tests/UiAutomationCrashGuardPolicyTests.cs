using ThreeFingerDragOnWindows.utils;
using Xunit;

namespace ThreeFingerDragOnWindows.Tests;

public sealed class UiAutomationCrashGuardPolicyTests{
    private const uint WmGetObject = 0x003D;
    private const int UiaRootObjectId = -25;

    [Theory]
    [InlineData(UiaRootObjectId)]
    [InlineData(unchecked((long) 0x00000000FFFFFFE7))]
    public void SuppressesUiaRootProviderRequestsAcrossPointerRepresentations(long objectId){
        Assert.True(UiAutomationCrashGuardPolicy.ShouldSuppress(WmGetObject, new IntPtr(objectId)));
    }

    [Theory]
    [InlineData(-4)]
    [InlineData(0)]
    [InlineData(1)]
    public void PreservesNonUiaObjectRequests(long objectId){
        Assert.False(UiAutomationCrashGuardPolicy.ShouldSuppress(WmGetObject, new IntPtr(objectId)));
    }

    [Fact]
    public void PreservesOtherWindowMessages(){
        Assert.False(UiAutomationCrashGuardPolicy.ShouldSuppress(0x00FF, new IntPtr(UiaRootObjectId)));
    }
}
