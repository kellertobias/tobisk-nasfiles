import { createRootRoute, Outlet } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import api from '../api/client';

export const Route = createRootRoute({
  component: RootLayout,
});

function RootLayout() {
  const { isLoading } = useQuery({
    queryKey: ['me'],
    queryFn: api.me,
    retry: false,
    staleTime: 5 * 60 * 1000,
  });

  if (isLoading) {
    return (
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          minHeight: '100vh',
          background: 'var(--color-bg)',
        }}
      >
        <div className="shimmer" style={{
          width: 200,
          height: 20,
          borderRadius: 'var(--radius-md)',
        }} />
      </div>
    );
  }

  return <Outlet />;
}
