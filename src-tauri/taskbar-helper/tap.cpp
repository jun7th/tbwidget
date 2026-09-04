#include <windows.h>
#include <ocidl.h>
#include <xamlom.h>

#ifdef GetCurrentTime
#undef GetCurrentTime
#endif

#include <winrt/base.h>
#include <winrt/Windows.UI.h>
#include <winrt/Windows.UI.Xaml.Media.h>
#include <winrt/Windows.UI.Xaml.Shapes.h>

#include <atomic>
#include <cstdio>
#include <cstdint>
#include <string>
#include <unordered_map>
#include <vector>

namespace
{
constexpr wchar_t WindowClassName[] = L"TBWidget.TaskbarTap.26100";
constexpr wchar_t ClearMessageName[] = L"TBWidget.TaskbarTap.Clear";
constexpr wchar_t RestoreMessageName[] = L"TBWidget.TaskbarTap.Restore";

constexpr CLSID TaskbarTapClsid = {
    0x8d5e8c44,
    0x1474,
    0x4e0d,
    {0xb4, 0xa8, 0x6e, 0x2b, 0x86, 0x58, 0x0d, 0x2a},
};

HMODULE g_module = nullptr;
std::atomic_ulong g_objectCount = 0;
std::atomic_ulong g_serverLocks = 0;
std::atomic_bool g_windowClassRegistered = false;

std::wstring Hex32(unsigned long value)
{
    wchar_t buffer[16]{};
    swprintf_s(buffer, L"0x%08lX", value);
    return buffer;
}

void LogLine(const std::wstring& message) noexcept
{
    try {
        wchar_t tempPath[MAX_PATH]{};
        const DWORD tempLength = GetTempPathW(MAX_PATH, tempPath);
        if (!tempLength || tempLength >= MAX_PATH) {
            OutputDebugStringW((message + L"\r\n").c_str());
            return;
        }

        std::wstring directory(tempPath, tempLength);
        if (!directory.empty() && directory.back() != L'\\') {
            directory.push_back(L'\\');
        }
        directory += L"tbwidget";
        CreateDirectoryW(directory.c_str(), nullptr);

        const std::wstring path = directory + L"\\tap-helper.log";
        const HANDLE file = CreateFileW(
            path.c_str(),
            FILE_APPEND_DATA,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            nullptr,
            OPEN_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            nullptr);
        if (file == INVALID_HANDLE_VALUE) {
            OutputDebugStringW((message + L"\r\n").c_str());
            return;
        }

        SYSTEMTIME now{};
        GetLocalTime(&now);
        wchar_t prefix[128]{};
        swprintf_s(
            prefix,
            L"[%04u-%02u-%02u %02u:%02u:%02u.%03u pid=%lu tid=%lu] ",
            now.wYear,
            now.wMonth,
            now.wDay,
            now.wHour,
            now.wMinute,
            now.wSecond,
            now.wMilliseconds,
            GetCurrentProcessId(),
            GetCurrentThreadId());
        const std::wstring line = std::wstring(prefix) + message + L"\r\n";
        const int utf8Length = WideCharToMultiByte(
            CP_UTF8,
            0,
            line.c_str(),
            static_cast<int>(line.size()),
            nullptr,
            0,
            nullptr,
            nullptr);
        if (utf8Length > 0) {
            std::string utf8(static_cast<size_t>(utf8Length), '\0');
            WideCharToMultiByte(
                CP_UTF8,
                0,
                line.c_str(),
                static_cast<int>(line.size()),
                utf8.data(),
                utf8Length,
                nullptr,
                nullptr);
            DWORD written{};
            WriteFile(file, utf8.data(), static_cast<DWORD>(utf8.size()), &written, nullptr);
        }
        CloseHandle(file);
        OutputDebugStringW(line.c_str());
    } catch (...) {
    }
}

struct Node
{
    InstanceHandle parent{};
    std::wstring type;
    std::wstring name;
};

struct TrackedShape
{
    winrt::Windows::UI::Xaml::Shapes::Rectangle rectangle{nullptr};
    winrt::Windows::UI::Xaml::Media::Brush originalFill{nullptr};
};

class TaskbarTap
    : public winrt::implements<TaskbarTap, IObjectWithSite, IVisualTreeServiceCallback2>
{
public:
    TaskbarTap()
    {
        ++g_objectCount;
        LogLine(L"TaskbarTap ctor objectCount=" + std::to_wstring(g_objectCount.load()));
    }

    ~TaskbarTap() noexcept
    {
        LogLine(L"TaskbarTap dtor begin");
        RestoreAll();
        Detach();
        --g_objectCount;
        LogLine(L"TaskbarTap dtor end objectCount=" + std::to_wstring(g_objectCount.load()));
    }

    HRESULT STDMETHODCALLTYPE SetSite(IUnknown* site) noexcept override
    {
        LogLine(std::wstring(L"SetSite begin site=") + (site ? L"non-null" : L"null"));
        try {
            const HRESULT restoreResult = RestoreAll();
            LogLine(L"SetSite RestoreAll result=" + Hex32(static_cast<unsigned long>(restoreResult)));
            const HRESULT detachResult = Detach();
            LogLine(L"SetSite Detach result=" + Hex32(static_cast<unsigned long>(detachResult)));
            if (FAILED(detachResult)) {
                return detachResult;
            }
            winrt::check_hresult(restoreResult);

            if (!site) {
                LogLine(L"SetSite null site complete");
                return S_OK;
            }

            m_site.copy_from(site);
            LogLine(L"SetSite QueryInterface IXamlDiagnostics begin");
            winrt::check_hresult(site->QueryInterface(m_diagnostics.put()));
            LogLine(L"SetSite QueryInterface IXamlDiagnostics ok");
            LogLine(L"SetSite QueryInterface IVisualTreeService begin");
            winrt::check_hresult(site->QueryInterface(m_visualTree.put()));
            LogLine(L"SetSite QueryInterface IVisualTreeService ok");

            LogLine(L"SetSite AdviseVisualTreeChange begin");
            winrt::check_hresult(m_visualTree->AdviseVisualTreeChange(this));
            m_advised = true;
            m_adviceOwnsReference = true;
            AddRef();
            LogLine(L"SetSite AdviseVisualTreeChange ok");

            m_clearMessage = RegisterWindowMessageW(ClearMessageName);
            m_restoreMessage = RegisterWindowMessageW(RestoreMessageName);
            LogLine(
                L"SetSite RegisterWindowMessage clear=" +
                std::to_wstring(m_clearMessage) +
                L" restore=" +
                std::to_wstring(m_restoreMessage));
            if (!m_clearMessage || !m_restoreMessage) {
                winrt::throw_last_error();
            }

            WNDCLASSW windowClass{};
            windowClass.lpfnWndProc = WindowProc;
            windowClass.hInstance = g_module;
            windowClass.lpszClassName = WindowClassName;
            if (!RegisterClassW(&windowClass) && GetLastError() != ERROR_CLASS_ALREADY_EXISTS) {
                LogLine(L"SetSite RegisterClass failed error=" + std::to_wstring(GetLastError()));
                winrt::throw_last_error();
            }
            g_windowClassRegistered.store(true);
            LogLine(L"SetSite RegisterClass ok");

            m_window = CreateWindowExW(
                0,
                WindowClassName,
                L"",
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                nullptr,
                g_module,
                this);
            if (!m_window) {
                LogLine(L"SetSite CreateWindowExW failed error=" + std::to_wstring(GetLastError()));
                winrt::throw_last_error();
            }
            m_windowOwnsReference = true;
            AddRef();
            LogLine(L"SetSite CreateWindowExW ok hwnd=0x" + Hex32(static_cast<unsigned long>(reinterpret_cast<uintptr_t>(m_window))));

            return S_OK;
        } catch (...) {
            const HRESULT error = winrt::to_hresult();
            LogLine(L"SetSite catch error=" + Hex32(static_cast<unsigned long>(error)));
            const HRESULT restoreResult = RestoreAll();
            const HRESULT detachResult = Detach();
            LogLine(
                L"SetSite catch cleanup restore=" +
                Hex32(static_cast<unsigned long>(restoreResult)) +
                L" detach=" +
                Hex32(static_cast<unsigned long>(detachResult)));
            if (FAILED(detachResult)) {
                return detachResult;
            }
            if (FAILED(restoreResult)) {
                return restoreResult;
            }
            return error;
        }
    }

    HRESULT STDMETHODCALLTYPE GetSite(REFIID iid, void** result) noexcept override
    {
        if (!result) {
            return E_POINTER;
        }
        *result = nullptr;

        try {
            return m_site ? m_site->QueryInterface(iid, result) : E_FAIL;
        } catch (...) {
            return winrt::to_hresult();
        }
    }

    HRESULT STDMETHODCALLTYPE OnVisualTreeChange(
        ParentChildRelation relation,
        VisualElement element,
        VisualMutationType mutationType) noexcept override
    {
        try {
            if (mutationType == Add) {
                Node node;
                node.parent = relation.Parent;
                if (element.Type) {
                    node.type = element.Type;
                }
                if (element.Name) {
                    node.name = element.Name;
                }
                if (node.type != L"Taskbar.TaskbarFrame" &&
                    !m_nodes.contains(relation.Parent)) {
                    return S_OK;
                }
                if (node.type == L"Taskbar.TaskbarFrame") {
                    LogLine(L"OnVisualTreeChange add TaskbarFrame handle=" + std::to_wstring(element.Handle));
                }
                m_nodes[element.Handle] = std::move(node);
                TrackIfEligible(element.Handle);
            } else if (mutationType == Remove) {
                RemoveNode(element.Handle);
            }
            return S_OK;
        } catch (...) {
            return winrt::to_hresult();
        }
    }

    HRESULT STDMETHODCALLTYPE OnElementStateChanged(
        InstanceHandle,
        VisualElementState,
        LPCWSTR) noexcept override
    {
        return S_OK;
    }

private:
    static bool TypeMatches(const Node& node, const wchar_t* type)
    {
        if (node.type == type) {
            return true;
        }

        const size_t typeLength = wcslen(type);
        return node.type.size() > typeLength &&
            node.type.compare(node.type.size() - typeLength, typeLength, type) == 0 &&
            node.type[node.type.size() - typeLength - 1] == L'.';
    }

    bool IsUnderTaskbarFrame(InstanceHandle handle) const
    {
        while (handle) {
            const auto node = m_nodes.find(handle);
            if (node == m_nodes.end()) {
                return false;
            }
            if (node->second.type == L"Taskbar.TaskbarFrame") {
                return true;
            }
            handle = node->second.parent;
        }
        return false;
    }

    bool IsEligibleShape(InstanceHandle handle) const
    {
        const auto rectangle = m_nodes.find(handle);
        if (rectangle == m_nodes.end() ||
            !TypeMatches(rectangle->second, L"Rectangle") ||
            (rectangle->second.name != L"BackgroundFill" &&
                rectangle->second.name != L"BackgroundStroke")) {
            return false;
        }

        const auto grid = m_nodes.find(rectangle->second.parent);
        if (grid == m_nodes.end() || !TypeMatches(grid->second, L"Grid")) {
            return false;
        }

        const auto background = m_nodes.find(grid->second.parent);
        return background != m_nodes.end() &&
            background->second.type == L"Taskbar.TaskbarBackground" &&
            IsUnderTaskbarFrame(background->second.parent);
    }

    void TrackIfEligible(InstanceHandle handle)
    {
        if (!IsEligibleShape(handle) || m_shapes.contains(handle)) {
            return;
        }

        ::IInspectable* inspectableAbi{};
        winrt::check_hresult(
            m_diagnostics->GetIInspectableFromHandle(handle, &inspectableAbi));
        winrt::Windows::Foundation::IInspectable inspectable{
            inspectableAbi,
            winrt::take_ownership_from_abi};
        auto rectangle = inspectable.as<winrt::Windows::UI::Xaml::Shapes::Rectangle>();

        TrackedShape shape{rectangle, rectangle.Fill()};
        auto [tracked, inserted] = m_shapes.emplace(handle, std::move(shape));
        if (inserted) {
            LogLine(L"TrackIfEligible tracked shape handle=" + std::to_wstring(handle) +
                L" totalShapes=" + std::to_wstring(m_shapes.size()));
        }
        if (inserted && m_clearRequested) {
            tracked->second.rectangle.Fill(TransparentBrush());
            LogLine(L"TrackIfEligible applied transparent brush to newly tracked shape handle=" + std::to_wstring(handle));
        }
    }

    winrt::Windows::UI::Xaml::Media::SolidColorBrush TransparentBrush()
    {
        if (!m_transparentBrush) {
            winrt::Windows::UI::Color transparent{};
            transparent.A = 0;
            transparent.R = 0;
            transparent.G = 0;
            transparent.B = 0;
            m_transparentBrush =
                winrt::Windows::UI::Xaml::Media::SolidColorBrush(transparent);
        }
        return m_transparentBrush;
    }

    void ClearAll()
    {
        m_clearRequested = true;
        LogLine(L"ClearAll begin shapes=" + std::to_wstring(m_shapes.size()));
        const auto brush = TransparentBrush();
        for (auto& [handle, shape] : m_shapes) {
            shape.rectangle.Fill(brush);
        }
        LogLine(L"ClearAll end");
    }

    HRESULT RestoreAll() noexcept
    {
        m_clearRequested = false;
        LogLine(L"RestoreAll begin shapes=" + std::to_wstring(m_shapes.size()));
        HRESULT result = S_OK;
        for (auto& [handle, shape] : m_shapes) {
            try {
                shape.rectangle.Fill(shape.originalFill);
            } catch (...) {
                if (SUCCEEDED(result)) {
                    result = winrt::to_hresult();
                }
            }
        }
        LogLine(L"RestoreAll end result=" + Hex32(static_cast<unsigned long>(result)));
        return result;
    }

    void RemoveNode(InstanceHandle handle)
    {
        std::vector<InstanceHandle> children;
        for (const auto& [candidate, node] : m_nodes) {
            if (node.parent == handle) {
                children.push_back(candidate);
            }
        }
        for (const InstanceHandle child : children) {
            RemoveNode(child);
        }

        const auto shape = m_shapes.find(handle);
        if (shape != m_shapes.end()) {
            if (m_clearRequested) {
                shape->second.rectangle.Fill(shape->second.originalFill);
            }
            m_shapes.erase(shape);
        }
        m_nodes.erase(handle);
    }

    static HRESULT UnregisterWindowClass() noexcept
    {
        if (!g_windowClassRegistered.load()) {
            return S_OK;
        }
        if (UnregisterClassW(WindowClassName, g_module)) {
            g_windowClassRegistered.store(false);
            return S_OK;
        }

        const DWORD error = GetLastError();
        if (error == ERROR_CLASS_DOES_NOT_EXIST) {
            g_windowClassRegistered.store(false);
            return S_OK;
        }
        return error == ERROR_CLASS_HAS_WINDOWS
            ? S_OK
            : error ? HRESULT_FROM_WIN32(error) : E_FAIL;
    }

    HRESULT Detach() noexcept
    {
        HRESULT result = S_OK;
        if (m_window) {
            const HWND window = m_window;
            if (DestroyWindow(window)) {
                if (m_window == window) {
                    m_window = nullptr;
                }
            } else {
                const DWORD error = GetLastError();
                result = error ? HRESULT_FROM_WIN32(error) : E_FAIL;
            }
        }

        bool releaseAdviceReference = false;
        if (m_advised && m_visualTree) {
            const HRESULT unadviseResult = m_visualTree->UnadviseVisualTreeChange(this);
            if (SUCCEEDED(unadviseResult)) {
                m_advised = false;
                releaseAdviceReference = m_adviceOwnsReference;
                m_adviceOwnsReference = false;
            } else if (SUCCEEDED(result)) {
                result = unadviseResult;
            }
        } else if (m_advised && SUCCEEDED(result)) {
            result = E_UNEXPECTED;
        }

        if (!m_window) {
            const HRESULT unregisterResult = UnregisterWindowClass();
            if (FAILED(unregisterResult) && SUCCEEDED(result)) {
                result = unregisterResult;
            }
        }

        if (!m_window && !m_advised) {
            m_shapes.clear();
            m_nodes.clear();
            m_transparentBrush = nullptr;
            m_visualTree = nullptr;
            m_diagnostics = nullptr;
            m_site = nullptr;
            m_clearMessage = 0;
            m_restoreMessage = 0;
        }

        if (releaseAdviceReference) {
            Release();
        }
        return result;
    }

    static LRESULT CALLBACK WindowProc(HWND window, UINT message, WPARAM wParam, LPARAM lParam) noexcept
    {
        TaskbarTap* self = reinterpret_cast<TaskbarTap*>(
            GetWindowLongPtrW(window, GWLP_USERDATA));
        if (message == WM_NCCREATE) {
            const auto create = reinterpret_cast<CREATESTRUCTW*>(lParam);
            self = static_cast<TaskbarTap*>(create->lpCreateParams);
            SetWindowLongPtrW(window, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(self));
        }

        if (self && message == WM_NCDESTROY) {
            const LRESULT result = DefWindowProcW(window, message, wParam, lParam);
            SetWindowLongPtrW(window, GWLP_USERDATA, 0);
            self->m_window = nullptr;
            const bool releaseWindowReference = self->m_windowOwnsReference;
            self->m_windowOwnsReference = false;
            if (releaseWindowReference) {
                self->Release();
            }
            return result;
        }

        try {
            if (self && message == self->m_clearMessage) {
                LogLine(L"WindowProc clear message");
                self->ClearAll();
                return 1;
            }
            if (self && message == self->m_restoreMessage) {
                LogLine(L"WindowProc restore message");
                return SUCCEEDED(self->RestoreAll()) ? 1 : 0;
            }
        } catch (...) {
            LogLine(L"WindowProc command catch");
            return 0;
        }

        return DefWindowProcW(window, message, wParam, lParam);
    }

    winrt::com_ptr<IUnknown> m_site;
    winrt::com_ptr<IXamlDiagnostics> m_diagnostics;
    winrt::com_ptr<IVisualTreeService> m_visualTree;
    std::unordered_map<InstanceHandle, Node> m_nodes;
    std::unordered_map<InstanceHandle, TrackedShape> m_shapes;
    winrt::Windows::UI::Xaml::Media::SolidColorBrush m_transparentBrush{nullptr};
    HWND m_window{};
    UINT m_clearMessage{};
    UINT m_restoreMessage{};
    bool m_advised{};
    bool m_clearRequested{};
    bool m_windowOwnsReference{};
    bool m_adviceOwnsReference{};
};

class ClassFactory : public winrt::implements<ClassFactory, IClassFactory>
{
public:
    ClassFactory()
    {
        ++g_objectCount;
        LogLine(L"ClassFactory ctor objectCount=" + std::to_wstring(g_objectCount.load()));
    }

    ~ClassFactory() noexcept
    {
        --g_objectCount;
        LogLine(L"ClassFactory dtor objectCount=" + std::to_wstring(g_objectCount.load()));
    }

    HRESULT STDMETHODCALLTYPE CreateInstance(
        IUnknown* outer,
        REFIID iid,
        void** result) noexcept override
    {
        LogLine(std::wstring(L"ClassFactory CreateInstance begin outer=") + (outer ? L"non-null" : L"null"));
        if (!result) {
            LogLine(L"ClassFactory CreateInstance E_POINTER");
            return E_POINTER;
        }
        *result = nullptr;
        if (outer) {
            LogLine(L"ClassFactory CreateInstance CLASS_E_NOAGGREGATION");
            return CLASS_E_NOAGGREGATION;
        }

        try {
            auto tap = winrt::make_self<TaskbarTap>();
            const HRESULT queryResult = tap->QueryInterface(iid, result);
            LogLine(L"ClassFactory CreateInstance QueryInterface result=" + Hex32(static_cast<unsigned long>(queryResult)));
            return queryResult;
        } catch (...) {
            const HRESULT error = winrt::to_hresult();
            LogLine(L"ClassFactory CreateInstance catch error=" + Hex32(static_cast<unsigned long>(error)));
            return error;
        }
    }

    HRESULT STDMETHODCALLTYPE LockServer(BOOL lock) noexcept override
    {
        if (lock) {
            ++g_serverLocks;
        } else if (g_serverLocks.load() != 0) {
            --g_serverLocks;
        }
        return S_OK;
    }
};
}

BOOL WINAPI DllMain(HMODULE module, DWORD reason, LPVOID)
{
    if (reason == DLL_PROCESS_ATTACH) {
        g_module = module;
        DisableThreadLibraryCalls(module);
    }
    return TRUE;
}

extern "C" HRESULT __stdcall DllGetClassObject(
    REFCLSID classId,
    REFIID iid,
    void** result)
{
    LogLine(L"DllGetClassObject begin");
    if (!result) {
        LogLine(L"DllGetClassObject E_POINTER");
        return E_POINTER;
    }
    *result = nullptr;
    if (classId != TaskbarTapClsid) {
        LogLine(L"DllGetClassObject CLASS_E_CLASSNOTAVAILABLE");
        return CLASS_E_CLASSNOTAVAILABLE;
    }

    try {
        auto factory = winrt::make_self<ClassFactory>();
        const HRESULT queryResult = factory->QueryInterface(iid, result);
        LogLine(L"DllGetClassObject QueryInterface result=" + Hex32(static_cast<unsigned long>(queryResult)));
        return queryResult;
    } catch (...) {
        const HRESULT error = winrt::to_hresult();
        LogLine(L"DllGetClassObject catch error=" + Hex32(static_cast<unsigned long>(error)));
        return error;
    }
}

extern "C" HRESULT __stdcall DllCanUnloadNow()
{
    return g_objectCount.load() == 0 &&
        g_serverLocks.load() == 0 &&
        !g_windowClassRegistered.load()
        ? S_OK
        : S_FALSE;
}
