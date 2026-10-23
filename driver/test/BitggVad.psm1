# Shared helpers for the bitggvad test scripts (run INSIDE the test VM, as admin).
# Creates and removes the root-enumerated device with SetupAPI, the same calls
# the future BitGG installer will make (docs/driver/SPEC.md section 4.1).

$script:HardwareId = 'ROOT\BitGG_VirtualAudio'

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class BitggSetupApi
{
    static readonly Guid MediaClass = new Guid("4d36e96c-e325-11ce-bfc1-08002be10318");
    const int DICD_GENERATE_ID = 1;
    const int SPDRP_HARDWAREID = 1;
    const int DIF_REGISTERDEVICE = 0x19;
    const int INSTALLFLAG_FORCE = 1;

    [StructLayout(LayoutKind.Sequential)]
    struct SP_DEVINFO_DATA { public int cbSize; public Guid ClassGuid; public int DevInst; public IntPtr Reserved; }

    [DllImport("setupapi.dll", SetLastError = true)]
    static extern IntPtr SetupDiCreateDeviceInfoList(ref Guid ClassGuid, IntPtr hwndParent);
    [DllImport("setupapi.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool SetupDiCreateDeviceInfoW(IntPtr set, string name, ref Guid cls, string desc, IntPtr hwnd, int flags, ref SP_DEVINFO_DATA data);
    [DllImport("setupapi.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool SetupDiSetDeviceRegistryPropertyW(IntPtr set, ref SP_DEVINFO_DATA data, int prop, byte[] buf, int size);
    [DllImport("setupapi.dll", SetLastError = true)]
    static extern bool SetupDiCallClassInstaller(int fn, IntPtr set, ref SP_DEVINFO_DATA data);
    [DllImport("setupapi.dll", SetLastError = true)]
    static extern bool SetupDiDestroyDeviceInfoList(IntPtr set);
    [DllImport("newdev.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool UpdateDriverForPlugAndPlayDevicesW(IntPtr hwnd, string hardwareId, string infPath, int flags, out bool reboot);

    // Creates a root device node with the hardware ID, then installs the INF on it.
    public static bool CreateRootDevice(string hardwareId, string infPath)
    {
        Guid cls = MediaClass;
        IntPtr set = SetupDiCreateDeviceInfoList(ref cls, IntPtr.Zero);
        if (set == new IntPtr(-1)) throw new System.ComponentModel.Win32Exception();
        try
        {
            var data = new SP_DEVINFO_DATA { cbSize = Marshal.SizeOf(typeof(SP_DEVINFO_DATA)) };
            if (!SetupDiCreateDeviceInfoW(set, "MEDIA", ref cls, null, IntPtr.Zero, DICD_GENERATE_ID, ref data))
                throw new System.ComponentModel.Win32Exception();
            byte[] ids = System.Text.Encoding.Unicode.GetBytes(hardwareId + "\0\0");
            if (!SetupDiSetDeviceRegistryPropertyW(set, ref data, SPDRP_HARDWAREID, ids, ids.Length))
                throw new System.ComponentModel.Win32Exception();
            if (!SetupDiCallClassInstaller(DIF_REGISTERDEVICE, set, ref data))
                throw new System.ComponentModel.Win32Exception();
        }
        finally { SetupDiDestroyDeviceInfoList(set); }

        bool reboot;
        if (!UpdateDriverForPlugAndPlayDevicesW(IntPtr.Zero, hardwareId, infPath, INSTALLFLAG_FORCE, out reboot))
            throw new System.ComponentModel.Win32Exception();
        return reboot;
    }
}
'@

function Assert-Admin {
    $p = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
    if (-not $p.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Run this script from an elevated PowerShell (Run as administrator).'
    }
}

function Test-TestSigning {
    $out = bcdedit /enum '{current}' | Out-String
    return $out -match 'testsigning\s+Yes'
}

function Get-BitggDevice {
    Get-PnpDevice -PresentOnly:$false -ErrorAction SilentlyContinue |
        Where-Object { $_.HardwareID -contains $script:HardwareId }
}

function Get-BitggDriverPackage {
    # oemNN.inf published names whose original name is bitggvad.inf
    $lines = pnputil /enum-drivers | Out-String
    $blocks = $lines -split "(\r?\n){2,}"
    foreach ($b in $blocks) {
        if ($b -match 'Original Name:\s+bitggvad\.inf' -and $b -match 'Published Name:\s+(oem\d+\.inf)') { $Matches[1] }
    }
}

Export-ModuleMember -Function Assert-Admin, Test-TestSigning, Get-BitggDevice, Get-BitggDriverPackage -Variable HardwareId
