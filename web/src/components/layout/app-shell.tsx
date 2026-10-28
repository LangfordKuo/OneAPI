import {
  Activity,
  Boxes,
  ChevronDown,
  FileText,
  KeyRound,
  LayoutDashboard,
  LogOut,
  Menu,
  Moon,
  Server,
  Settings,
  Sparkles,
  Sun,
} from 'lucide-react';
import { useTheme } from 'next-themes';
import { useState, type ReactNode } from 'react';
import { NavLink, useLocation } from 'react-router-dom';
import { Button } from '@/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet';
import { useLiveMetrics } from '@/hooks/useLiveMetrics';
import { cn } from '@/lib/utils';

const NAV_ITEMS = [
  { to: '/', label: '仪表盘', icon: LayoutDashboard, end: true },
  { to: '/channels', label: '渠道', icon: Server },
  { to: '/keys', label: 'API Keys', icon: KeyRound },
  { to: '/logs', label: '日志', icon: FileText },
  { to: '/playground', label: 'Playground', icon: Sparkles },
  { to: '/settings', label: '设置', icon: Settings },
];

const PAGE_TITLES: Record<string, string> = {
  '/': '仪表盘',
  '/channels': '渠道管理',
  '/keys': 'API Keys',
  '/logs': '请求日志',
  '/playground': 'Playground',
  '/settings': '设置',
};

interface AppShellProps {
  siteName: string;
  version?: string;
  username: string;
  onLogout: () => void;
  children: ReactNode;
}

function SidebarContent({
  siteName,
  version,
  onNavigate,
}: {
  siteName: string;
  version?: string;
  onNavigate?: () => void;
}) {
  return (
    <div className="flex h-full flex-col">
      <div className="flex h-14 shrink-0 items-center gap-2.5 border-b px-4">
        <div className="flex size-8 items-center justify-center rounded-lg bg-primary text-primary-foreground">
          <Boxes className="size-4" />
        </div>
        <div className="min-w-0">
          <div className="truncate text-sm font-semibold">{siteName}</div>
          <div className="truncate text-[11px] text-muted-foreground">
            AI API Gateway
          </div>
        </div>
      </div>

      <nav className="flex-1 space-y-1 overflow-y-auto p-3">
        {NAV_ITEMS.map((item) => (
          <NavLink
            key={item.to}
            to={item.to}
            end={item.end}
            onClick={onNavigate}
            className={({ isActive }) =>
              cn(
                'flex items-center gap-2.5 rounded-lg px-3 py-2 text-sm font-medium transition-colors',
                isActive
                  ? 'bg-sidebar-accent text-sidebar-accent-foreground'
                  : 'text-muted-foreground hover:bg-sidebar-accent/60 hover:text-foreground',
              )
            }
          >
            <item.icon className="size-4 shrink-0" />
            {item.label}
          </NavLink>
        ))}
      </nav>

      <div className="shrink-0 border-t p-3">
        <div className="flex items-center justify-between rounded-lg bg-sidebar-accent/50 px-3 py-2 text-xs">
          <span className="text-muted-foreground">
            {version ? `v${version}` : 'OneAPI'}
          </span>
          <span className="flex items-center gap-1.5 text-emerald-600 dark:text-emerald-400">
            <span className="size-1.5 rounded-full bg-current" />
            运行中
          </span>
        </div>
      </div>
    </div>
  );
}

function LiveRateBadge() {
  const live = useLiveMetrics();
  const active = live.global_tok_s > 0;

  return (
    <div
      className="hidden items-center gap-1.5 rounded-full border bg-card px-2.5 py-1 text-xs text-muted-foreground sm:flex"
      title="全站实时输出速度"
    >
      <Activity
        className={cn(
          'size-3.5',
          active ? 'text-emerald-500' : 'text-muted-foreground/50',
        )}
      />
      <span className="font-mono tabular-nums text-foreground">
        {live.global_tok_s.toFixed(1)}
      </span>
      <span>tok/s</span>
    </div>
  );
}

export function AppShell({
  siteName,
  version,
  username,
  onLogout,
  children,
}: AppShellProps) {
  const location = useLocation();
  const [mobileOpen, setMobileOpen] = useState(false);
  const { resolvedTheme, setTheme } = useTheme();
  const isDark = resolvedTheme === 'dark';
  const title = PAGE_TITLES[location.pathname] ?? '管理后台';

  return (
    <div className="min-h-full bg-muted/30">
      <aside className="fixed inset-y-0 left-0 z-40 hidden w-64 flex-col border-r bg-sidebar text-sidebar-foreground lg:flex">
        <SidebarContent siteName={siteName} version={version} />
      </aside>

      <Sheet open={mobileOpen} onOpenChange={setMobileOpen}>
        <SheetContent side="left" className="w-72 p-0">
          <SheetHeader className="sr-only">
            <SheetTitle>主导航</SheetTitle>
            <SheetDescription>在管理后台页面之间切换</SheetDescription>
          </SheetHeader>
          <SidebarContent
            siteName={siteName}
            version={version}
            onNavigate={() => setMobileOpen(false)}
          />
        </SheetContent>
      </Sheet>

      <div className="lg:pl-64">
        <header className="sticky top-0 z-30 flex h-14 items-center gap-3 border-b bg-background/80 px-4 backdrop-blur lg:px-6">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="lg:hidden"
            onClick={() => setMobileOpen(true)}
            title="打开导航"
          >
            <Menu />
          </Button>

          <div className="min-w-0 flex-1">
            <h1 className="truncate text-sm font-semibold">{title}</h1>
          </div>

          <LiveRateBadge />

          <Button
            type="button"
            variant="ghost"
            size="icon"
            onClick={() => setTheme(isDark ? 'light' : 'dark')}
            title={isDark ? '切换到亮色模式' : '切换到暗色模式'}
          >
            {isDark ? <Sun /> : <Moon />}
          </Button>

          <DropdownMenu>
            <DropdownMenuTrigger
              render={<Button type="button" variant="ghost" size="sm" />}
            >
              <span className="flex size-6 items-center justify-center rounded-full bg-muted text-xs font-medium">
                {username.slice(0, 1).toUpperCase()}
              </span>
              <span className="hidden max-w-[120px] truncate sm:inline">
                {username}
              </span>
              <ChevronDown className="size-3.5 text-muted-foreground" />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-48">
              <DropdownMenuGroup>
                <DropdownMenuLabel>管理员</DropdownMenuLabel>
                <div className="truncate px-1.5 pb-1 text-sm font-medium">
                  {username}
                </div>
              </DropdownMenuGroup>
              <DropdownMenuSeparator />
              <DropdownMenuItem onClick={onLogout}>
                <LogOut className="size-4" />
                退出登录
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </header>

        <main className="mx-auto w-full max-w-[1600px] p-4 lg:p-6">
          {children}
        </main>
      </div>
    </div>
  );
}
